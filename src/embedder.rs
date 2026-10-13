//! Safe wrapper over kiln's embedding API: one model, one context, one vector per call.

use crate::ffi::*;
use anyhow::{anyhow, bail, Result};
use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr;

#[allow(dead_code)]
pub struct Embedder {
    model: *mut kiln_model,
    ctx: *mut kiln_context,
    pub dims: usize,
    pub backend: String,
    pub has_projector: bool,
    pub supports_images: bool,
    pub supports_audio: bool,
    pub last_stats: kiln_embed_stats,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Query,
    Document,
}

fn last_error() -> String {
    unsafe {
        let p = kiln_last_error();
        if p.is_null() {
            String::new()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

fn check(s: kiln_status, what: &str) -> Result<()> {
    if s == KILN_OK {
        return Ok(());
    }
    let name = unsafe { CStr::from_ptr(kiln_status_string(s)).to_string_lossy().into_owned() };
    Err(anyhow!("{what}: {name} ({s}) {}", last_error()))
}

fn cstr(s: &str) -> Result<CString> {
    CString::new(s.replace('\0', " ")).map_err(|e| anyhow!("bad string: {e}"))
}

fn fixed(buf: &[std::os::raw::c_char]) -> String {
    unsafe { CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned() }
}

#[allow(dead_code)]
impl Embedder {
    pub fn version() -> String {
        unsafe { CStr::from_ptr(kiln_version()).to_string_lossy().into_owned() }
    }

    /// Loads the text model, optionally its image/audio projector, and a context that can
    /// hold `n_ctx` tokens (the longest single text that can be embedded).
    pub fn load(model_path: &Path, mmproj: Option<&Path>, n_ctx: i32, threads: i32) -> Result<Self> {
        unsafe { kiln_set_log_level(KILN_LOG_ERROR) };
        let mut model: *mut kiln_model = ptr::null_mut();
        let mut mp = unsafe { kiln_model_params_default() };
        mp.backend = KILN_BACKEND_AUTO;
        mp.n_threads = threads;
        let cpath = cstr(&model_path.to_string_lossy())?;
        check(unsafe { kiln_model_load(cpath.as_ptr(), &mp, &mut model) }, "model load")?;

        let mut info: kiln_model_info = unsafe { std::mem::zeroed() };
        check(unsafe { kiln_model_get_info(model, &mut info) }, "model info")?;
        if !info.is_embedding_model {
            unsafe { kiln_model_free(model) };
            bail!("{} is not an embedding model", model_path.display());
        }

        let mut pinfo = kiln_projector_info::default();
        let mut has_projector = false;
        if let Some(mm) = mmproj {
            let c = cstr(&mm.to_string_lossy())?;
            let pp = unsafe { kiln_projector_params_default() };
            let s = unsafe { kiln_model_load_projector(model, c.as_ptr(), &pp, &mut pinfo) };
            if let Err(e) = check(s, "projector load") {
                unsafe { kiln_model_free(model) };
                return Err(e);
            }
            has_projector = true;
        }

        let mut cp = unsafe { kiln_context_params_default() };
        cp.n_ctx = n_ctx;
        let mut ctx: *mut kiln_context = ptr::null_mut();
        if let Err(e) = check(unsafe { kiln_context_create(model, &cp, &mut ctx) }, "context") {
            unsafe { kiln_model_free(model) };
            return Err(e);
        }

        Ok(Self {
            model,
            ctx,
            dims: info.n_embd as usize,
            backend: fixed(&info.backend_name),
            has_projector,
            supports_images: pinfo.supports_images,
            supports_audio: pinfo.supports_audio,
            last_stats: kiln_embed_stats::default(),
        })
    }

    fn opts(kind: Kind) -> kiln_embed_options {
        let mut o = unsafe { kiln_embed_options_default() };
        o.kind = match kind {
            Kind::Query => KILN_EMBED_QUERY,
            Kind::Document => KILN_EMBED_DOCUMENT,
        };
        o.normalize = true;
        o.dimensions = 0;
        o
    }

    /// Embeds a text. Returns Ok(None) when the text does not fit the context
    /// (the caller should split it), Err on everything else.
    pub fn embed_text(&mut self, text: &str, kind: Kind) -> Result<Option<Vec<f32>>> {
        let c = cstr(text)?;
        let mut out = vec![0f32; self.dims];
        let o = Self::opts(kind);
        let mut st = kiln_embed_stats::default();
        let s = unsafe { kiln_embed(self.ctx, c.as_ptr(), &o, out.as_mut_ptr(), self.dims as i32, &mut st) };
        self.last_stats = st;
        if s == KILN_ERR_CONTEXT_FULL {
            return Ok(None);
        }
        check(s, "embed")?;
        Ok(Some(out))
    }

    /// Embeds one image or audio file alone (no text). Needs a projector.
    pub fn embed_media_file(&mut self, path: &Path) -> Result<Option<Vec<f32>>> {
        if !self.has_projector {
            bail!("no projector loaded; pass --mmproj to embed images and audio");
        }
        let c = cstr(&path.to_string_lossy())?;
        let media = kiln_media { path: c.as_ptr(), data: ptr::null(), size: 0 };
        let mut out = vec![0f32; self.dims];
        let o = Self::opts(Kind::Document);
        let mut st = kiln_embed_stats::default();
        let s = unsafe {
            kiln_embed_media(self.ctx, ptr::null(), &media, 1, &o, out.as_mut_ptr(), self.dims as i32, &mut st)
        };
        self.last_stats = st;
        if s == KILN_ERR_CONTEXT_FULL {
            return Ok(None);
        }
        check(s, "embed media")?;
        Ok(Some(out))
    }
}

impl Drop for Embedder {
    fn drop(&mut self) {
        unsafe {
            if !self.ctx.is_null() {
                kiln_context_free(self.ctx);
            }
            if !self.model.is_null() {
                kiln_model_free(self.model);
            }
        }
    }
}
