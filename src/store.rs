//! SQLite index: files, their chunks (text + vector), and an FTS5 keyword index.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub struct Store {
    pub conn: Connection,
}

#[allow(dead_code)]
pub struct FileRow {
    pub id: i64,
    pub mtime: i64,
    pub size: i64,
    pub kind: String,
    pub chunks: i64,
}

pub struct Hit {
    pub path: String,
    pub kind: String,
    pub locator: i64,
    pub text: String,
    pub score: f32,
    pub semantic: f32,
    pub keyword: bool,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).ok();
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS files (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                kind TEXT NOT NULL,
                mtime INTEGER NOT NULL,
                size INTEGER NOT NULL,
                indexed_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS chunks (
                id INTEGER PRIMARY KEY,
                file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                locator INTEGER NOT NULL,
                text TEXT NOT NULL,
                vec BLOB NOT NULL
            );
            CREATE INDEX IF NOT EXISTS chunks_file ON chunks(file_id);
            CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
                text, content='chunks', content_rowid='id', tokenize='unicode61'
            );
            CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
                INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
            END;
            CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
            END;
            "#,
        )?;
        Ok(Self { conn })
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Folders that were indexed, in the order they were first added.
    pub fn roots(&self) -> Vec<String> {
        self.meta("roots")
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok())
            .unwrap_or_default()
    }

    pub fn add_root(&self, root: &str) -> Result<()> {
        let mut r = self.roots();
        if !r.iter().any(|x| x == root) {
            r.push(root.to_string());
            self.set_meta("roots", &serde_json::to_string(&r)?)?;
        }
        Ok(())
    }

    /// Stops searching a folder: forgets it and every file under it that no other indexed
    /// folder covers. The files themselves are not touched. Returns how many were forgotten.
    pub fn remove_root(&mut self, root: &str) -> Result<usize> {
        let rest: Vec<String> = self.roots().into_iter().filter(|r| r != root).collect();
        let covered = |p: &str| rest.iter().any(|r| p == r || p.starts_with(&format!("{}/", r.trim_end_matches('/'))));
        let gone: Vec<String> = self.files_under(root)?.into_iter().filter(|p| !covered(p)).collect();
        let tx = self.conn.transaction()?;
        for p in &gone {
            tx.execute("DELETE FROM files WHERE path = ?1", [p])?;
        }
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('roots', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [serde_json::to_string(&rest)?],
        )?;
        tx.commit()?;
        Ok(gone.len())
    }

    pub fn file(&self, path: &str) -> Result<Option<FileRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT f.id, f.mtime, f.size, f.kind, (SELECT COUNT(*) FROM chunks c WHERE c.file_id = f.id)
                 FROM files f WHERE f.path = ?1",
                [path],
                |r| Ok(FileRow { id: r.get(0)?, mtime: r.get(1)?, size: r.get(2)?, kind: r.get(3)?, chunks: r.get(4)? }),
            )
            .optional()?)
    }

    pub fn remove_file(&self, path: &str) -> Result<()> {
        self.conn.execute("DELETE FROM files WHERE path = ?1", [path])?;
        Ok(())
    }

    /// Replaces a file and all its chunks in one transaction.
    pub fn put_file(
        &mut self,
        path: &str,
        kind: &str,
        mtime: i64,
        size: i64,
        chunks: &[(i64, String, Vec<f32>)],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM files WHERE path = ?1", [path])?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        tx.execute(
            "INSERT INTO files(path, kind, mtime, size, indexed_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![path, kind, mtime, size, now],
        )?;
        let file_id = tx.last_insert_rowid();
        {
            let mut st = tx.prepare("INSERT INTO chunks(file_id, locator, text, vec) VALUES (?1, ?2, ?3, ?4)")?;
            for (loc, text, vec) in chunks {
                st.execute(params![file_id, loc, text, vec_to_blob(vec)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Paths of every indexed file under `root`.
    pub fn files_under(&self, root: &str) -> Result<Vec<String>> {
        let prefix = if root.ends_with('/') { root.to_string() } else { format!("{root}/") };
        let mut st = self.conn.prepare("SELECT path FROM files WHERE path = ?1 OR path LIKE ?2 ESCAPE '\\'")?;
        let like = format!("{}%", prefix.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let rows = st.query_map(params![root, like], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn counts(&self) -> Result<(i64, i64, Vec<(String, i64)>)> {
        let files: i64 = self.conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        let chunks: i64 = self.conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
        let mut st = self.conn.prepare("SELECT kind, COUNT(*) FROM files GROUP BY kind ORDER BY kind")?;
        let kinds = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        Ok((files, chunks, kinds))
    }

    /// Ranks every chunk by dot product with `query` (vectors are unit length, so this is
    /// cosine similarity), adds a boost for chunks whose text contains the query's words,
    /// and returns the best chunk of each of the top `limit` files.
    pub fn search(&self, query: &[f32], query_text: &str, limit: usize, boost: f32) -> Result<Vec<Hit>> {
        let keyword_hits = self.keyword_rowids(query_text)?;

        let mut st = self.conn.prepare(
            "SELECT c.id, c.file_id, c.locator, c.text, c.vec, f.path, f.kind
             FROM chunks c JOIN files f ON f.id = c.file_id",
        )?;
        let mut best: std::collections::HashMap<i64, Hit> = std::collections::HashMap::new();
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        for row in rows {
            let (id, file_id, locator, text, blob, path, kind) = row?;
            let sem = dot_blob(query, &blob);
            let kw = keyword_hits.contains(&id);
            let score = sem + if kw { boost } else { 0.0 };
            let better = best.get(&file_id).map(|h| score > h.score).unwrap_or(true);
            if better {
                best.insert(file_id, Hit { path, kind, locator, text, score, semantic: sem, keyword: kw });
            }
        }
        let mut hits: Vec<Hit> = best.into_values().collect();
        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(limit);
        Ok(hits)
    }

    /// Chunk ids whose text contains every word of the query (FTS5, unicode61 tokenizer).
    fn keyword_rowids(&self, query_text: &str) -> Result<std::collections::HashSet<i64>> {
        let terms: Vec<String> = query_text
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
            .filter(|t| t.chars().count() >= 2)
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect();
        let mut out = std::collections::HashSet::new();
        if terms.is_empty() {
            return Ok(out);
        }
        let q = terms.join(" AND ");
        let mut st = self.conn.prepare("SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ?1")?;
        let rows = match st.query_map([q], |r| r.get::<_, i64>(0)) {
            Ok(r) => r,
            Err(_) => return Ok(out), // an unparsable query only loses the boost
        };
        for r in rows.flatten() {
            out.insert(r);
        }
        Ok(out)
    }
}

pub fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(v.len() * 4);
    for x in v {
        b.extend_from_slice(&x.to_le_bytes());
    }
    b
}

fn dot_blob(q: &[f32], blob: &[u8]) -> f32 {
    let n = q.len().min(blob.len() / 4);
    let mut s = 0f32;
    for i in 0..n {
        let x = f32::from_le_bytes([blob[4 * i], blob[4 * i + 1], blob[4 * i + 2], blob[4 * i + 3]]);
        s += q[i] * x;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(v: &[f32]) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect()
    }

    fn temp_store(tag: &str) -> (Store, std::path::PathBuf) {
        let p = std::env::temp_dir().join(format!("infind-test-{tag}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        (Store::open(&p).unwrap(), p)
    }

    #[test]
    fn ranks_by_similarity_and_keeps_best_chunk_per_file() {
        let (mut s, p) = temp_store("rank");
        s.put_file("/a.txt", "text", 1, 1, &[(1, "alpha one".into(), unit(&[1.0, 0.0, 0.0])), (9, "alpha two".into(), unit(&[0.9, 0.1, 0.0]))]).unwrap();
        s.put_file("/b.txt", "text", 1, 1, &[(1, "beta".into(), unit(&[0.0, 1.0, 0.0]))]).unwrap();
        let hits = s.search(&unit(&[1.0, 0.0, 0.0]), "zzzz", 10, 0.15).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, "/a.txt");
        assert_eq!(hits[0].locator, 1);
        assert!(hits[0].score > hits[1].score);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn exact_words_boost_a_chunk() {
        let (mut s, p) = temp_store("boost");
        s.put_file("/code.txt", "text", 1, 1, &[(1, "error AUTH-E7731 expired".into(), unit(&[1.0, 0.6]))]).unwrap();
        s.put_file("/other.txt", "text", 1, 1, &[(1, "nothing here at all".into(), unit(&[1.0, 0.2]))]).unwrap();
        // Without the words the second file is closer by about 0.12; the default boost is 0.15.
        let hits = s.search(&unit(&[1.0, 0.0]), "AUTH-E7731", 10, 0.15).unwrap();
        assert_eq!(hits[0].path, "/code.txt");
        assert!(hits[0].keyword);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn replacing_and_removing_files() {
        let (mut s, p) = temp_store("replace");
        s.put_file("/a.txt", "text", 1, 1, &[(1, "old words".into(), unit(&[1.0, 0.0]))]).unwrap();
        s.put_file("/a.txt", "text", 2, 1, &[(1, "new words".into(), unit(&[0.0, 1.0]))]).unwrap();
        let (files, chunks, _) = s.counts().unwrap();
        assert_eq!((files, chunks), (1, 1));
        assert_eq!(s.file("/a.txt").unwrap().unwrap().mtime, 2);
        // The full-text index forgot the old words.
        assert!(s.search(&unit(&[1.0, 0.0]), "old", 5, 0.5).unwrap()[0].keyword == false);
        s.remove_file("/a.txt").unwrap();
        assert_eq!(s.counts().unwrap().1, 0);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn roots_are_remembered_once() {
        let (s, p) = temp_store("roots");
        s.add_root("/home/x").unwrap();
        s.add_root("/home/x").unwrap();
        s.add_root("/data").unwrap();
        assert_eq!(s.roots(), vec!["/home/x".to_string(), "/data".to_string()]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn removing_a_folder_keeps_files_another_folder_covers() {
        let (mut s, p) = temp_store("forget");
        for f in ["/h/d/a.txt", "/h/d/t/b.txt", "/h/e/c.txt"] {
            s.put_file(f, "text", 1, 1, &[(1, "hello there".into(), unit(&[1.0]))]).unwrap();
        }
        for r in ["/h/d", "/h/d/t", "/h/e"] {
            s.add_root(r).unwrap();
        }
        // The parent goes; its subfolder is still indexed on its own.
        assert_eq!(s.remove_root("/h/d").unwrap(), 1);
        assert_eq!(s.roots(), vec!["/h/d/t".to_string(), "/h/e".to_string()]);
        assert!(s.file("/h/d/a.txt").unwrap().is_none());
        assert!(s.file("/h/d/t/b.txt").unwrap().is_some());
        assert_eq!(s.counts().unwrap().1, 2);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn files_under_matches_folders_not_prefixes() {
        let (mut s, p) = temp_store("under");
        for f in ["/h/a/x.txt", "/h/ab/y.txt"] {
            s.put_file(f, "text", 1, 1, &[(1, "hello there".into(), unit(&[1.0]))]).unwrap();
        }
        assert_eq!(s.files_under("/h/a").unwrap(), vec!["/h/a/x.txt".to_string()]);
        let _ = std::fs::remove_file(p);
    }
}
