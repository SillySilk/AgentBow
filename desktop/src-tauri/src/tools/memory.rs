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
) -> Result<String> {
    let findings_text = findings.join("\n");

    // The `embedding` column is kept for schema compatibility but is always NULL.
    let conn = db.lock().map_err(|_| anyhow!("DB lock poisoned"))?;
    conn.execute(
        "INSERT INTO memories (outcome, task_desc, findings)
         VALUES (?1, ?2, ?3)",
        params![outcome, task_desc, findings_text],
    )?;
    let id = conn.last_insert_rowid();

    Ok(format!("Memory #{} stored (outcome: {})", id, outcome))
}

pub async fn memory_retrieve(
    db: &MemoryDb,
    query: &str,
    limit: usize,
) -> Result<String> {
    struct Row {
        id: i64,
        #[allow(dead_code)]
        created_at: i64,
        outcome: String,
        task_desc: String,
        findings: String,
    }

    let fts_q = fts_query(query);

    // ── FTS5 search (primary path, no model needed) ──────────────────────────
    // bm25() returns negative values; ORDER BY rank gives best matches first.
    let fts_rows: Vec<Row> = {
        let conn = db.lock().map_err(|_| anyhow!("DB lock poisoned"))?;

        if fts_q.is_empty() {
            // Empty query — return most recent memories
            let mut stmt = conn.prepare(
                "SELECT id, created_at, outcome, task_desc, findings
                 FROM memories ORDER BY created_at DESC LIMIT ?1"
            )?;
            let rows: Vec<Row> = stmt.query_map(params![limit as i64], |r| Ok(Row {
                id: r.get(0)?,
                created_at: r.get(1)?,
                outcome: r.get(2)?,
                task_desc: r.get(3)?,
                findings: r.get(4)?,
            }))?
            .filter_map(|r| r.ok())
            .collect();
            rows
        } else {
            let mut stmt = conn.prepare(
                "SELECT m.id, m.created_at, m.outcome, m.task_desc, m.findings
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
                }))?
                .filter_map(|r| r.ok())
                .collect();

            // If FTS matched nothing, fall back to recency
            if rows.is_empty() {
                let mut stmt2 = conn.prepare(
                    "SELECT id, created_at, outcome, task_desc, findings
                     FROM memories ORDER BY created_at DESC LIMIT ?1"
                )?;
                let fallback: Vec<Row> = stmt2.query_map(params![limit as i64], |r| Ok(Row {
                    id: r.get(0)?,
                    created_at: r.get(1)?,
                    outcome: r.get(2)?,
                    task_desc: r.get(3)?,
                    findings: r.get(4)?,
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

    let top: Vec<String> = fts_rows
        .iter()
        .take(limit)
        .map(|row| {
            format!(
                "Memory #{}\nTask: {}\nOutcome: {}\nFindings: {}",
                row.id, row.task_desc, row.outcome, row.findings
            )
        })
        .collect();

    Ok(top.join("\n\n---\n\n"))
}
