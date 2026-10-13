//! Raw C declarations for the part of kiln.h that infind uses (embedding models only).
//! Hand-written to mirror `kiln.h` (these structs are the same in kiln 0.3.1 and 0.4.0);
//! keep layouts in sync when kiln changes them.
#![allow(non_camel_case_types, dead_code)]

use std::os::raw::{c_char, c_int};

pub type kiln_status = c_int;
pub const KILN_OK: kiln_status = 0;
pub const KILN_ERR_CONTEXT_FULL: kiln_status = 5;
pub const KILN_ERR_NOT_SUPPORTED: kiln_status = 8;

pub const KILN_BACKEND_AUTO: c_int = 0;

pub const KILN_EMBED_RAW: c_int = 0;
pub const KILN_EMBED_QUERY: c_int = 1;
pub const KILN_EMBED_DOCUMENT: c_int = 2;

pub const KILN_LOG_ERROR: c_int = 0;

#[repr(C)]
pub struct kiln_model {
    _private: [u8; 0],
}
#[repr(C)]
pub struct kiln_context {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct kiln_model_params {
    pub backend: c_int,
    pub n_gpu_layers: i32,
    pub n_threads: i32,
    pub use_mmap: bool,
    pub use_mlock: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct kiln_model_info {
    pub backend_used: c_int,
    pub backend_name: [c_char; 64],
    pub arch: [c_char; 32],
    pub quant: [c_char; 16],
    pub n_params: i64,
    pub n_ctx_train: i32,
    pub n_vocab: i32,
    pub n_gpu_layers_used: i32,
    pub model_bytes: u64,
    pub supports_thinking: bool,
    pub is_decision_model: bool,
    pub is_embedding_model: bool,
    pub n_embd: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct kiln_context_params {
    pub n_ctx: i32,
    pub n_batch: i32,
    pub flash_attn: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct kiln_embed_options {
    pub kind: c_int,
    pub title: *const c_char,
    pub dimensions: i32,
    pub normalize: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct kiln_embed_stats {
    pub tokens: i32,
    pub media_tokens: i32,
    pub dimensions: i32,
    pub prefixed: bool,
    pub media_ms: f64,
    pub ms: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct kiln_projector_params {
    pub image_max_tokens: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct kiln_projector_info {
    pub supports_images: bool,
    pub supports_audio: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct kiln_media {
    pub path: *const c_char,
    pub data: *const u8,
    pub size: usize,
}

extern "C" {
    pub fn kiln_last_error() -> *const c_char;
    pub fn kiln_status_string(s: kiln_status) -> *const c_char;
    pub fn kiln_version() -> *const c_char;

    pub fn kiln_model_params_default() -> kiln_model_params;
    pub fn kiln_model_load(
        path: *const c_char,
        params: *const kiln_model_params,
        out: *mut *mut kiln_model,
    ) -> kiln_status;
    pub fn kiln_model_free(m: *mut kiln_model);
    pub fn kiln_model_get_info(m: *const kiln_model, out: *mut kiln_model_info) -> kiln_status;

    pub fn kiln_context_params_default() -> kiln_context_params;
    pub fn kiln_context_create(
        m: *mut kiln_model,
        params: *const kiln_context_params,
        out: *mut *mut kiln_context,
    ) -> kiln_status;
    pub fn kiln_context_free(c: *mut kiln_context);

    pub fn kiln_embed_options_default() -> kiln_embed_options;
    pub fn kiln_embed(
        c: *mut kiln_context,
        text: *const c_char,
        opts: *const kiln_embed_options,
        out: *mut f32,
        out_cap: i32,
        stats: *mut kiln_embed_stats,
    ) -> kiln_status;

    pub fn kiln_projector_params_default() -> kiln_projector_params;
    pub fn kiln_model_load_projector(
        m: *mut kiln_model,
        path: *const c_char,
        params: *const kiln_projector_params,
        out: *mut kiln_projector_info,
    ) -> kiln_status;
    pub fn kiln_embed_media(
        c: *mut kiln_context,
        text: *const c_char,
        media: *const kiln_media,
        n_media: i32,
        opts: *const kiln_embed_options,
        out: *mut f32,
        out_cap: i32,
        stats: *mut kiln_embed_stats,
    ) -> kiln_status;

    pub fn kiln_set_log_level(level: c_int);
}
