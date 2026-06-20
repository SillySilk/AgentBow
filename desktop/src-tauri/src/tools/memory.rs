use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// ── Database setup ────────────────────────────────────────────────────────────

pub type MemoryDb = Arc<Mutex<Connection>>;

pub fn open_db(workspace_root: &str) -> Result<MemoryDb> {
    let db_path: PathBuf = [workspace_root, "memory.db"].iter().collect();
    let conn = Connection::open(&db_path)
        .map_err(|e| anyhow!("Failed to open memory DB at {:?}: {}", db_path, e))?;

    // Main table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memories (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at  INTEGER NOT NULL DEFAULT (strftime('%s','now')),
            outcome     TEXT NOT NULL,
            task_desc   TEXT NOT NULL,
            findings    TEXT NOT NULL,
            embedding   BLOB
        );

        -- FTS5 full-text search index (BM25 ranking, no second model needed)
        CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
            task_desc,
            findings,
            content='memories',
            content_rowid='id'
        );

        -- Keep FTS5 in sync when rows are inserted
        CREATE TRIGGER IF NOT EXISTS memories_ai
        AFTER INSERT ON memories BEGIN
            INSERT INTO memories_fts(rowid, task_desc, findings)
            VALUES (new.id, new.task_desc, new.findings);
        END;

        -- Backfill any existing rows that predate the FTS table
        INSERT OR IGNORE INTO memories_fts(rowid, task_desc, findings)
        SELECT id, task_desc, findings FROM memories
        WHERE id NOT IN (SELECT rowid FROM memories_fts);",
    )?;

    Ok(Arc::new(Mutex::new(conn)))
}

// ── Optional: vector embeddings via LM Studio ─────────────────────────────────
// Used only when an embedding model is loaded alongside the chat model.
// Memory works fully without this — FTS5 is the primary retrieval path.

async fn try_embed(text: &str, lm_studio_url: &str) -> Option<Vec<f32>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    let body = serde_json::json!({
        "input": text,
        "model": "text-embedding-nomic-embed-text-v1.5"
    });

    let resp = client
        .post(format!("{}/v1/embeddings", lm_studio_url))
        .json(&body)
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let v: serde_json::Value = resp.json().await.ok()?;
    let embedding: Vec<f32> = v["data"][0]["embedding"]
        .as_array()?
        .iter()
        .filter_map(|x| x.as_f64().map(|f| f as f32))
        .collect();

    if embedding.is_empty() { None } else { Some(embedding) }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 { 0.0 } else { dot / (norm_a * norm_b) }
}

fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn blob_to_vec(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

// ── Sanitize FTS5 query ───────────────────────────────────────────────────────
// FTS5 MATCH syntax is strict — strip special chars to avoid parse errors.

fn fts_query(text: &str) -> String {
    // Keep alphanumeric, spaces, hyphens. Wrap each word in quotes for phrase safety.
    text.split_whitespace()
        .map(|w| {
            let clean: String = w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '-')
                .collect();
            clean
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Public API ────────────────────────────────────────────────────────────────

pub async fn memory_store(
    db: &MemoryDb,
    task_desc: &str,
    outcome: &str,
    findings: &[&str],
    lm_studio_url: &str,
) -> Result<String> {
    let findings_text = findings.join("\n");

    // Try embedding — silently skip if no embedding model is loaded
    let embedding_blob: Option<Vec<u8>> = {
        let embed_input = format!("{} {}", task_desc, findings_text);
        try_embed(&embed_input, lm_studio_url).await.map(|v| vec_to_blob(&v))
    };

    let conn = db.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
    conn.execute(
        "INSERT INTO memories (outcome, task_desc, findings, embedding)
         VALUES (?1, ?2, ?3, ?4)",
        params![outcome, task_desc, findings_text, embedding_blob],
    )?;
    let id = conn.last_insert_rowid();

    Ok(format!("Memory #{} stored (outcome: {})", id, outcome))
}

pub async fn memory_retrieve(
    db: &MemoryDb,
    query: &str,
    limit: usize,
    lm_studio_url: &str,
) -> Result<String> {
    struct Row {
        id: i64,
        #[allow(dead_code)]
        created_at: i64,
        outcome: String,
        task_desc: String,
        findings: String,
        embedding: Option<Vec<u8>>,
    }

    let fts_q = fts_query(query);

    // ── FTS5 search (primary path, no model needed) ──────────────────────────
    // bm25() returns negative values; ORDER BY rank gives best matches first.
    let fts_rows: Vec<Row> = {
        let conn = db.lock().map_err(|_| anyhow!("DB lock poisoned"))?;

        if fts_q.is_empty() {
            // Empty query — return most recent memories
            let mut stmt = conn.prepare(
                "SELECT id, created_at, outcome, task_desc, findings, embedding
                 FROM memories ORDER BY created_at DESC LIMIT ?1"
            )?;
            let rows: Vec<Row> = stmt.query_map(params![limit as i64], |r| Ok(Row {
                id: r.get(0)?,
                created_at: r.get(1)?,
                outcome: r.get(2)?,
                task_desc: r.get(3)?,
                findings: r.get(4)?,
                embedding: r.get(5)?,
            }))?
            .filter_map(|r| r.ok())
            .collect();
            rows
        } else {
            let mut stmt = conn.prepare(
                "SELECT m.id, m.created_at, m.outcome, m.task_desc, m.findings, m.embedding
                 FROM memories_fts
                 JOIN memories m ON memories_fts.rowid = m.id
                 WHERE memories_fts MATCH ?1
                 ORDER BY bm25(memories_fts)
                 LIMIT ?2"
            )?;
            let rows: Vec<Row> = stmt
                .query_map(params![fts_q, limit as i64 * 3], |r| Ok(Row {
                    id: r.get(0)?,
                    created_at: r.get(1)?,
                    outcome: r.get(2)?,
                    task_desc: r.get(3)?,
                    findings: r.get(4)?,
                    embedding: r.get(5)?,
                }))?
                .filter_map(|r| r.ok())
                .collect();

            // If FTS matched nothing, fall back to recency
            if rows.is_empty() {
                let mut stmt2 = conn.prepare(
                    "SELECT id, created_at, outcome, task_desc, findings, embedding
                     FROM memories ORDER BY created_at DESC LIMIT ?1"
                )?;
                let fallback: Vec<Row> = stmt2.query_map(params![limit as i64], |r| Ok(Row {
                    id: r.get(0)?,
                    created_at: r.get(1)?,
                    outcome: r.get(2)?,
                    task_desc: r.get(3)?,
                    findings: r.get(4)?,
                    embedding: r.get(5)?,
                }))?
                .filter_map(|r| r.ok())
                .collect();
                fallback
            } else {
                rows
            }
        }
        // conn dropped here
    };

    if fts_rows.is_empty() {
        return Ok("No memories stored yet.".to_string());
    }

    // ── Optional: re-rank with embeddings if available ───────────────────────
    let query_vec = try_embed(query, lm_studio_url).await;

    let mut scored: Vec<(f32, &Row)> = fts_rows
        .iter()
        .map(|row| {
            let score = match (&query_vec, &row.embedding) {
                (Some(qv), Some(rv)) => cosine_similarity(qv, &blob_to_vec(rv)),
                _ => 0.0,
            };
            (score, row)
        })
        .collect();

    // Only re-sort if we actually got embedding scores; otherwise preserve FTS order
    if query_vec.is_some() {
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    }

    let top: Vec<String> = scored
        .iter()
        .take(limit)
        .map(|(score, row)| {
            let score_note = if *score > 0.0 {
                format!(" (similarity: {:.2})", score)
            } else {
                String::new()
            };
            format!(
                "Memory #{}{}\nTask: {}\nOutcome: {}\nFindings: {}",
                row.id, score_note, row.task_desc, row.outcome, row.findings
            )
        })
        .collect();

    Ok(top.join("\n\n---\n\n"))
}
