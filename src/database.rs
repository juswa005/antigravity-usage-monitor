use crate::models::{DailyTokenBucket, SnapshotHistoryRow, UsageSnapshot, WeeklySummaryRow};
use crate::transcripts::iso_week_for_day;
use rusqlite::{params, Connection};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

pub struct HistoryDatabase {
    conn: Connection,
}

impl HistoryDatabase {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 3000)?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                collected_at TEXT NOT NULL,
                auth_state TEXT NOT NULL,
                connectivity TEXT NOT NULL,
                account_email TEXT,
                active_model TEXT,
                available_models_count INTEGER NOT NULL DEFAULT 0,
                tokens_5h_in INTEGER NOT NULL DEFAULT 0,
                tokens_5h_cache INTEGER NOT NULL DEFAULT 0,
                tokens_5h_out INTEGER NOT NULL DEFAULT 0,
                tokens_today_in INTEGER NOT NULL DEFAULT 0,
                tokens_today_cache INTEGER NOT NULL DEFAULT 0,
                tokens_today_out INTEGER NOT NULL DEFAULT 0,
                tokens_all_in INTEGER NOT NULL DEFAULT 0,
                tokens_all_cache INTEGER NOT NULL DEFAULT 0,
                tokens_all_out INTEGER NOT NULL DEFAULT 0,
                remote_quota_pct REAL,
                plan_tier TEXT,
                collection_error TEXT
            );

            CREATE TABLE IF NOT EXISTS model_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                snapshot_id INTEGER NOT NULL,
                model_id TEXT NOT NULL,
                display_name TEXT NOT NULL,
                available INTEGER NOT NULL,
                steps_all_time INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                remote_quota_pct REAL,
                FOREIGN KEY(snapshot_id) REFERENCES snapshots(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS daily_aggregates (
                day_local TEXT PRIMARY KEY,
                planner_steps INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                thinking_tokens INTEGER NOT NULL DEFAULT 0,
                response_tokens INTEGER NOT NULL DEFAULT 0,
                conversations_active INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_snapshots_collected_at ON snapshots(collected_at);
            CREATE INDEX IF NOT EXISTS idx_model_snapshots_snapshot_id ON model_snapshots(snapshot_id);
            ",
        )?;
        // Safe migration for pre-existing DBs created before plan_tier column was added
        let _ = self
            .conn
            .execute("ALTER TABLE snapshots ADD COLUMN plan_tier TEXT", []);
        Ok(())
    }

    pub fn record_snapshot(
        &mut self,
        snap: &UsageSnapshot,
        max_snapshots: usize,
        force_insert: bool,
    ) -> rusqlite::Result<bool> {
        let tx = self.conn.transaction()?;

        {
            let mut upsert = tx.prepare(
                "INSERT INTO daily_aggregates (
                    day_local, planner_steps, input_tokens, cache_read_tokens,
                    output_tokens, thinking_tokens, response_tokens,
                    conversations_active, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(day_local) DO UPDATE SET
                    planner_steps = MAX(daily_aggregates.planner_steps, excluded.planner_steps),
                    input_tokens = MAX(daily_aggregates.input_tokens, excluded.input_tokens),
                    cache_read_tokens = MAX(daily_aggregates.cache_read_tokens, excluded.cache_read_tokens),
                    output_tokens = MAX(daily_aggregates.output_tokens, excluded.output_tokens),
                    thinking_tokens = MAX(daily_aggregates.thinking_tokens, excluded.thinking_tokens),
                    response_tokens = MAX(daily_aggregates.response_tokens, excluded.response_tokens),
                    conversations_active = MAX(daily_aggregates.conversations_active, excluded.conversations_active),
                    updated_at = excluded.updated_at",
            )?;
            let updated_at = snap.collected_at.to_rfc3339();
            for b in &snap.daily_buckets {
                upsert.execute(params![
                    b.day_local,
                    b.planner_steps as i64,
                    b.input_tokens as i64,
                    b.cache_read_tokens as i64,
                    b.output_tokens as i64,
                    b.thinking_tokens as i64,
                    b.response_tokens as i64,
                    b.conversations_active as i64,
                    updated_at,
                ])?;
            }
        }

        let should_insert = if force_insert {
            true
        } else {
            struct LatestRow {
                auth: String,
                conn: String,
                model: Option<String>,
                t_in: i64,
                t_out: i64,
                avail_cnt: i64,
                quota: Option<f64>,
            }
            let last: Option<LatestRow> = tx
                .query_row(
                    "SELECT auth_state, connectivity, active_model, tokens_all_in, tokens_all_out, available_models_count, remote_quota_pct
                     FROM snapshots ORDER BY id DESC LIMIT 1",
                    [],
                    |r| {
                        Ok(LatestRow {
                            auth: r.get(0)?,
                            conn: r.get(1)?,
                            model: r.get(2)?,
                            t_in: r.get(3)?,
                            t_out: r.get(4)?,
                            avail_cnt: r.get(5)?,
                            quota: r.get(6)?,
                        })
                    },
                )
                .ok();
            let avail_cnt = snap.models.iter().filter(|m| m.available).count() as i64;
            match last {
                Some(prev) => {
                    let q_changed = match (prev.quota, snap.quota.remaining_pct) {
                        (Some(x), Some(y)) => (x - y).abs() > 0.05,
                        (None, None) => false,
                        _ => true,
                    };
                    prev.auth != snap.account.auth_state.as_label()
                        || prev.conn != snap.connectivity.as_label()
                        || prev.model != snap.active_model_label
                        || prev.t_in != snap.window_all_time.input_tokens as i64
                        || prev.t_out != snap.window_all_time.output_tokens as i64
                        || prev.avail_cnt != avail_cnt
                        || q_changed
                }
                None => true,
            }
        };

        if should_insert {
            let avail_cnt = snap.models.iter().filter(|m| m.available).count() as i64;
            tx.execute(
                "INSERT INTO snapshots (
                    collected_at, auth_state, connectivity, account_email, active_model,
                    available_models_count, tokens_5h_in, tokens_5h_cache, tokens_5h_out,
                    tokens_today_in, tokens_today_cache, tokens_today_out,
                    tokens_all_in, tokens_all_cache, tokens_all_out,
                    remote_quota_pct, plan_tier, collection_error
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                params![
                    snap.collected_at.to_rfc3339(),
                    snap.account.auth_state.as_label(),
                    snap.connectivity.as_label(),
                    snap.account.email,
                    snap.active_model_label,
                    avail_cnt,
                    snap.window_5h.input_tokens as i64,
                    snap.window_5h.cache_read_tokens as i64,
                    snap.window_5h.output_tokens as i64,
                    snap.window_today.input_tokens as i64,
                    snap.window_today.cache_read_tokens as i64,
                    snap.window_today.output_tokens as i64,
                    snap.window_all_time.input_tokens as i64,
                    snap.window_all_time.cache_read_tokens as i64,
                    snap.window_all_time.output_tokens as i64,
                    snap.quota.remaining_pct,
                    snap.quota.plan_tier,
                    snap.collection_error,
                ],
            )?;
            let snapshot_id = tx.last_insert_rowid();

            let mut ins_model = tx.prepare(
                "INSERT INTO model_snapshots (
                    snapshot_id, model_id, display_name, available,
                    steps_all_time, input_tokens, cache_read_tokens, output_tokens, remote_quota_pct
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for m in &snap.models {
                ins_model.execute(params![
                    snapshot_id,
                    m.id,
                    m.display_name,
                    if m.available { 1i64 } else { 0i64 },
                    m.steps_all_time as i64,
                    m.input_tokens as i64,
                    m.cache_read_tokens as i64,
                    m.output_tokens as i64,
                    m.remote_quota_pct,
                ])?;
            }

            if max_snapshots > 0 {
                tx.execute(
                    "DELETE FROM snapshots WHERE id NOT IN (
                        SELECT id FROM snapshots ORDER BY id DESC LIMIT ?1
                    )",
                    params![max_snapshots as i64],
                )?;
            }
        }

        tx.commit()?;
        Ok(should_insert)
    }

    pub fn query_daily_summaries(
        &self,
        limit_days: usize,
    ) -> rusqlite::Result<Vec<DailyTokenBucket>> {
        let mut stmt = self.conn.prepare(
            "SELECT day_local, planner_steps, input_tokens, cache_read_tokens,
                    output_tokens, thinking_tokens, response_tokens, conversations_active
             FROM daily_aggregates
             ORDER BY day_local DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit_days as i64], |r| {
            Ok(DailyTokenBucket {
                day_local: r.get(0)?,
                planner_steps: r.get::<_, i64>(1)? as u64,
                input_tokens: r.get::<_, i64>(2)? as u64,
                cache_read_tokens: r.get::<_, i64>(3)? as u64,
                output_tokens: r.get::<_, i64>(4)? as u64,
                thinking_tokens: r.get::<_, i64>(5)? as u64,
                response_tokens: r.get::<_, i64>(6)? as u64,
                conversations_active: r.get::<_, i64>(7)? as u64,
            })
        })?;
        let mut out: Vec<DailyTokenBucket> = rows.flatten().collect();
        out.reverse();
        Ok(out)
    }

    pub fn query_weekly_summaries(
        &self,
        limit_weeks: usize,
    ) -> rusqlite::Result<Vec<WeeklySummaryRow>> {
        let all_days = self.query_daily_summaries(400)?;
        let mut weeks: BTreeMap<String, WeeklySummaryRow> = BTreeMap::new();
        for d in all_days {
            if let Some((wk, _date)) = iso_week_for_day(&d.day_local) {
                let entry = weeks.entry(wk.clone()).or_insert_with(|| WeeklySummaryRow {
                    iso_week: wk,
                    start_day: d.day_local.clone(),
                    end_day: d.day_local.clone(),
                    active_days: 0,
                    planner_steps: 0,
                    input_tokens: 0,
                    cache_read_tokens: 0,
                    output_tokens: 0,
                });
                if d.day_local < entry.start_day {
                    entry.start_day = d.day_local.clone();
                }
                if d.day_local > entry.end_day {
                    entry.end_day = d.day_local.clone();
                }
                entry.active_days += 1;
                entry.planner_steps += d.planner_steps;
                entry.input_tokens += d.input_tokens;
                entry.cache_read_tokens += d.cache_read_tokens;
                entry.output_tokens += d.output_tokens;
            }
        }
        let mut list: Vec<WeeklySummaryRow> = weeks.into_values().collect();
        if list.len() > limit_weeks {
            list = list.split_off(list.len() - limit_weeks);
        }
        Ok(list)
    }

    pub fn query_recent_snapshots(
        &self,
        limit: usize,
    ) -> rusqlite::Result<Vec<SnapshotHistoryRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, collected_at, auth_state, connectivity, active_model,
                    available_models_count, tokens_5h_in, tokens_5h_cache, tokens_5h_out,
                    tokens_today_in, tokens_today_out, tokens_all_in, tokens_all_cache,
                    tokens_all_out, remote_quota_pct, collection_error
             FROM snapshots
             ORDER BY id DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            let t5_in = r.get::<_, i64>(6)? as u64;
            let t5_cache = r.get::<_, i64>(7)? as u64;
            let t5_out = r.get::<_, i64>(8)? as u64;
            let td_in = r.get::<_, i64>(9)? as u64;
            let td_out = r.get::<_, i64>(10)? as u64;
            let ta_in = r.get::<_, i64>(11)? as u64;
            let ta_cache = r.get::<_, i64>(12)? as u64;
            let ta_out = r.get::<_, i64>(13)? as u64;
            Ok(SnapshotHistoryRow {
                id: r.get(0)?,
                collected_at: r.get(1)?,
                auth_state: r.get(2)?,
                connectivity: r.get(3)?,
                active_model: r.get(4)?,
                available_models_count: r.get::<_, i64>(5)? as u64,
                tokens_5h_fresh: t5_in + t5_out,
                tokens_5h_combined: t5_in + t5_cache + t5_out,
                tokens_today_fresh: td_in + td_out,
                tokens_all_combined: ta_in + ta_cache + ta_out,
                remote_quota_pct: r.get(14)?,
                collection_error: r.get(15)?,
            })
        })?;
        Ok(rows.flatten().collect())
    }

    pub fn query_latest_model_comparison(&self) -> rusqlite::Result<HashMap<String, (u64, u64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM snapshots ORDER BY id DESC LIMIT 2")?;
        let ids: Vec<i64> = stmt.query_map([], |r| r.get(0))?.flatten().collect();
        if ids.len() < 2 {
            return Ok(HashMap::new());
        }
        let curr = self.model_tokens_for_snapshot(ids[0])?;
        let prev = self.model_tokens_for_snapshot(ids[1])?;
        let mut out = HashMap::new();
        for (name, c_tok) in curr {
            let p_tok = prev.get(&name).copied().unwrap_or(0);
            out.insert(name, (p_tok, c_tok.saturating_sub(p_tok)));
        }
        Ok(out)
    }

    fn model_tokens_for_snapshot(
        &self,
        snapshot_id: i64,
    ) -> rusqlite::Result<HashMap<String, u64>> {
        let mut stmt = self.conn.prepare(
            "SELECT display_name, input_tokens, cache_read_tokens, output_tokens
             FROM model_snapshots WHERE snapshot_id = ?1",
        )?;
        let rows = stmt.query_map(params![snapshot_id], |r| {
            let name: String = r.get(0)?;
            let total = (r.get::<_, i64>(1)? + r.get::<_, i64>(2)? + r.get::<_, i64>(3)?) as u64;
            Ok((name, total))
        })?;
        Ok(rows.flatten().collect())
    }

    pub fn snapshot_count(&self) -> rusqlite::Result<u64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM snapshots", [], |r| r.get::<_, i64>(0))
            .map(|v| v as u64)
    }
}
