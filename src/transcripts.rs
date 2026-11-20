use crate::models::{ConversationUsage, DailyTokenBucket, TokenWindowStats};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct ModelUsageTotals {
    pub steps_all_time: u64,
    pub steps_5h: u64,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    pub response_tokens: u64,
}

#[derive(Debug, Clone, Default)]
pub struct LocalTranscriptAggregation {
    pub window_5h: TokenWindowStats,
    pub window_today: TokenWindowStats,
    pub window_7d: TokenWindowStats,
    pub window_30d: TokenWindowStats,
    pub window_all_time: TokenWindowStats,
    pub daily_buckets: Vec<DailyTokenBucket>,
    pub model_totals: HashMap<String, ModelUsageTotals>,
    pub top_conversations: Vec<ConversationUsage>,
    pub sources_checked: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct ConvMeta {
    title: String,
    workspace: String,
}

#[derive(Debug, Clone, Default)]
pub struct GenStepMeta {
    pub model_label: Option<String>,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    pub response_tokens: u64,
    pub created_at: Option<DateTime<Utc>>,
}

impl GenStepMeta {
    pub fn has_data(&self) -> bool {
        self.model_label.is_some()
            || self.input_tokens > 0
            || self.cache_read_tokens > 0
            || self.output_tokens > 0
            || self.thinking_tokens > 0
            || self.response_tokens > 0
    }
}

pub fn collect_local_transcripts(
    antigravity_dir: &Path,
    now: DateTime<Utc>,
) -> LocalTranscriptAggregation {
    let mut agg = LocalTranscriptAggregation::default();
    agg.window_5h.window_name = "5-Hour Rolling".to_string();
    agg.window_today.window_name = "Today".to_string();
    agg.window_7d.window_name = "7-Day Rolling".to_string();
    agg.window_30d.window_name = "30-Day Rolling".to_string();
    agg.window_all_time.window_name = "All-Time Local".to_string();

    let conv_meta = load_conversation_metadata(antigravity_dir);

    let brain_dir = antigravity_dir.join("brain");
    let Ok(entries) = fs::read_dir(&brain_dir) else {
        return agg;
    };

    let cutoff_5h = now - Duration::hours(5);
    let cutoff_7d = now - Duration::days(7);
    let cutoff_30d = now - Duration::days(30);
    let local_today = now.with_timezone(&Local).date_naive();
    let today_start_utc = Local
        .from_local_datetime(&local_today.and_hms_opt(0, 0, 0).expect("valid midnight"))
        .single()
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or(now - Duration::hours(24));

    let mut convs_5h = HashSet::new();
    let mut convs_today = HashSet::new();
    let mut convs_7d = HashSet::new();
    let mut convs_30d = HashSet::new();
    let mut convs_all = HashSet::new();

    let mut daily_map: HashMap<String, (DailyTokenBucket, HashSet<String>)> = HashMap::new();
    let mut conv_usages = Vec::new();
    let mut transcripts_found = 0usize;
    let mut conv_dbs_found = 0usize;

    for entry in entries.flatten() {
        let cid = entry.file_name().to_string_lossy().into_owned();
        let transcript_path = entry
            .path()
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl");
        if !transcript_path.is_file() {
            continue;
        }
        transcripts_found += 1;

        let conv_db_path = antigravity_dir
            .join("conversations")
            .join(format!("{cid}.db"));
        let gen_metas = if conv_db_path.is_file() {
            conv_dbs_found += 1;
            read_gen_metadata(&conv_db_path)
        } else {
            Vec::new()
        };

        let Ok(file) = File::open(&transcript_path) else {
            continue;
        };
        let reader = BufReader::new(file);

        let mut planner_ord = 0usize;
        let mut conv_steps = 0u64;
        let mut conv_in = 0u64;
        let mut conv_cache = 0u64;
        let mut conv_out = 0u64;
        let mut conv_last_activity: Option<DateTime<Utc>> = None;
        let mut conv_model_counts: HashMap<String, u64> = HashMap::new();

        for line in reader.lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(obj) = serde_json::from_str::<Value>(&line) else {
                continue;
            };

            let step_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let has_tokens =
                obj.get("input_tokens").is_some() || obj.get("output_tokens").is_some();
            if step_type != "PLANNER_RESPONSE" && !has_tokens {
                continue;
            }

            let gmeta = gen_metas.get(planner_ord).cloned().unwrap_or_default();
            planner_ord += 1;

            let in_tok = obj
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .filter(|&v| v > 0)
                .unwrap_or(gmeta.input_tokens);
            let cache_tok = obj
                .get("cache_read_tokens")
                .and_then(|v| v.as_u64())
                .filter(|&v| v > 0)
                .unwrap_or(gmeta.cache_read_tokens);
            let out_tok = obj
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .filter(|&v| v > 0)
                .unwrap_or_else(|| {
                    if gmeta.output_tokens > 0 {
                        gmeta.output_tokens
                    } else {
                        gmeta.thinking_tokens + gmeta.response_tokens
                    }
                });
            let is_error = obj.get("status").and_then(|v| v.as_str()) == Some("ERROR");

            let created_at = gmeta.created_at.or_else(|| {
                obj.get("created_at")
                    .and_then(|v| v.as_str())
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&Utc))
            });

            let model_name = gmeta
                .model_label
                .unwrap_or_else(|| "Unknown / Unattributed".to_string());
            let think_tok = gmeta.thinking_tokens;
            let resp_tok = if gmeta.response_tokens > 0 || gmeta.thinking_tokens > 0 {
                gmeta.response_tokens
            } else {
                out_tok
            };

            *conv_model_counts.entry(model_name.clone()).or_insert(0) += 1;

            conv_steps += 1;
            conv_in += in_tok;
            conv_cache += cache_tok;
            conv_out += out_tok;
            if let Some(ts) = created_at {
                if conv_last_activity.map(|cur| ts > cur).unwrap_or(true) {
                    conv_last_activity = Some(ts);
                }
            }

            // All-time window
            add_step_to_window(
                &mut agg.window_all_time,
                in_tok,
                cache_tok,
                out_tok,
                think_tok,
                resp_tok,
                is_error,
            );
            convs_all.insert(cid.clone());

            // Model totals
            let m_entry = agg.model_totals.entry(model_name).or_default();
            m_entry.steps_all_time += 1;
            m_entry.input_tokens += in_tok;
            m_entry.cache_read_tokens += cache_tok;
            m_entry.output_tokens += out_tok;
            m_entry.thinking_tokens += think_tok;
            m_entry.response_tokens += resp_tok;

            if let Some(ts) = created_at {
                if ts >= cutoff_5h {
                    add_step_to_window(
                        &mut agg.window_5h,
                        in_tok,
                        cache_tok,
                        out_tok,
                        think_tok,
                        resp_tok,
                        is_error,
                    );
                    convs_5h.insert(cid.clone());
                    m_entry.steps_5h += 1;
                }
                if ts >= today_start_utc {
                    add_step_to_window(
                        &mut agg.window_today,
                        in_tok,
                        cache_tok,
                        out_tok,
                        think_tok,
                        resp_tok,
                        is_error,
                    );
                    convs_today.insert(cid.clone());
                }
                if ts >= cutoff_7d {
                    add_step_to_window(
                        &mut agg.window_7d,
                        in_tok,
                        cache_tok,
                        out_tok,
                        think_tok,
                        resp_tok,
                        is_error,
                    );
                    convs_7d.insert(cid.clone());
                }
                if ts >= cutoff_30d {
                    add_step_to_window(
                        &mut agg.window_30d,
                        in_tok,
                        cache_tok,
                        out_tok,
                        think_tok,
                        resp_tok,
                        is_error,
                    );
                    convs_30d.insert(cid.clone());
                }

                let day_key = ts.with_timezone(&Local).format("%Y-%m-%d").to_string();
                let entry_day = daily_map.entry(day_key.clone()).or_insert_with(|| {
                    (
                        DailyTokenBucket {
                            day_local: day_key,
                            planner_steps: 0,
                            input_tokens: 0,
                            cache_read_tokens: 0,
                            output_tokens: 0,
                            thinking_tokens: 0,
                            response_tokens: 0,
                            conversations_active: 0,
                        },
                        HashSet::new(),
                    )
                });
                entry_day.0.planner_steps += 1;
                entry_day.0.input_tokens += in_tok;
                entry_day.0.cache_read_tokens += cache_tok;
                entry_day.0.output_tokens += out_tok;
                entry_day.0.thinking_tokens += think_tok;
                entry_day.0.response_tokens += resp_tok;
                entry_day.1.insert(cid.clone());
            }
        }

        // Include any additional SQLite gen_metadata steps when transcript.jsonl was compacted
        if planner_ord < gen_metas.len() {
            for gmeta in &gen_metas[planner_ord..] {
                let in_tok = gmeta.input_tokens;
                let cache_tok = gmeta.cache_read_tokens;
                let out_tok = if gmeta.output_tokens > 0 {
                    gmeta.output_tokens
                } else {
                    gmeta.thinking_tokens + gmeta.response_tokens
                };
                let think_tok = gmeta.thinking_tokens;
                let resp_tok = if gmeta.response_tokens > 0 || gmeta.thinking_tokens > 0 {
                    gmeta.response_tokens
                } else {
                    out_tok
                };
                let model_name = gmeta
                    .model_label
                    .clone()
                    .unwrap_or_else(|| "Unknown / Unattributed".to_string());

                *conv_model_counts.entry(model_name.clone()).or_insert(0) += 1;
                conv_steps += 1;
                conv_in += in_tok;
                conv_cache += cache_tok;
                conv_out += out_tok;
                if let Some(ts) = gmeta.created_at {
                    if conv_last_activity.map(|cur| ts > cur).unwrap_or(true) {
                        conv_last_activity = Some(ts);
                    }
                }

                add_step_to_window(
                    &mut agg.window_all_time,
                    in_tok,
                    cache_tok,
                    out_tok,
                    think_tok,
                    resp_tok,
                    false,
                );
                convs_all.insert(cid.clone());

                let m_entry = agg.model_totals.entry(model_name).or_default();
                m_entry.steps_all_time += 1;
                m_entry.input_tokens += in_tok;
                m_entry.cache_read_tokens += cache_tok;
                m_entry.output_tokens += out_tok;
                m_entry.thinking_tokens += think_tok;
                m_entry.response_tokens += resp_tok;

                if let Some(ts) = gmeta.created_at {
                    if ts >= cutoff_5h {
                        add_step_to_window(
                            &mut agg.window_5h,
                            in_tok,
                            cache_tok,
                            out_tok,
                            think_tok,
                            resp_tok,
                            false,
                        );
                        convs_5h.insert(cid.clone());
                        m_entry.steps_5h += 1;
                    }
                    if ts >= today_start_utc {
                        add_step_to_window(
                            &mut agg.window_today,
                            in_tok,
                            cache_tok,
                            out_tok,
                            think_tok,
                            resp_tok,
                            false,
                        );
                        convs_today.insert(cid.clone());
                    }
                    if ts >= cutoff_7d {
                        add_step_to_window(
                            &mut agg.window_7d,
                            in_tok,
                            cache_tok,
                            out_tok,
                            think_tok,
                            resp_tok,
                            false,
                        );
                        convs_7d.insert(cid.clone());
                    }
                    if ts >= cutoff_30d {
                        add_step_to_window(
                            &mut agg.window_30d,
                            in_tok,
                            cache_tok,
                            out_tok,
                            think_tok,
                            resp_tok,
                            false,
                        );
                        convs_30d.insert(cid.clone());
                    }

                    let day_key = ts.with_timezone(&Local).format("%Y-%m-%d").to_string();
                    let entry_day = daily_map.entry(day_key.clone()).or_insert_with(|| {
                        (
                            DailyTokenBucket {
                                day_local: day_key,
                                planner_steps: 0,
                                input_tokens: 0,
                                cache_read_tokens: 0,
                                output_tokens: 0,
                                thinking_tokens: 0,
                                response_tokens: 0,
                                conversations_active: 0,
                            },
                            HashSet::new(),
                        )
                    });
                    entry_day.0.planner_steps += 1;
                    entry_day.0.input_tokens += in_tok;
                    entry_day.0.cache_read_tokens += cache_tok;
                    entry_day.0.output_tokens += out_tok;
                    entry_day.0.thinking_tokens += think_tok;
                    entry_day.0.response_tokens += resp_tok;
                    entry_day.1.insert(cid.clone());
                }
            }
        }

        if conv_steps > 0 {
            let meta = conv_meta.get(&cid).cloned().unwrap_or_default();
            let primary_model = conv_model_counts
                .into_iter()
                .max_by_key(|(_, cnt)| *cnt)
                .map(|(m, _)| m);
            let title = if meta.title.is_empty() {
                cid.chars().take(8).collect()
            } else {
                meta.title
            };
            conv_usages.push(ConversationUsage {
                conversation_id: cid,
                title,
                workspace: meta.workspace,
                primary_model,
                last_activity: conv_last_activity,
                planner_steps: conv_steps,
                input_tokens: conv_in,
                cache_read_tokens: conv_cache,
                output_tokens: conv_out,
            });
        }
    }

    agg.window_5h.conversations_active = convs_5h.len() as u64;
    agg.window_today.conversations_active = convs_today.len() as u64;
    agg.window_7d.conversations_active = convs_7d.len() as u64;
    agg.window_30d.conversations_active = convs_30d.len() as u64;
    agg.window_all_time.conversations_active = convs_all.len() as u64;

    let mut daily_buckets: Vec<DailyTokenBucket> = daily_map
        .into_values()
        .map(|(mut b, cset)| {
            b.conversations_active = cset.len() as u64;
            b
        })
        .collect();
    daily_buckets.sort_by(|a, b| a.day_local.cmp(&b.day_local));
    agg.daily_buckets = daily_buckets;

    conv_usages.sort_by(|a, b| {
        (b.input_tokens + b.output_tokens)
            .cmp(&(a.input_tokens + a.output_tokens))
            .then_with(|| b.last_activity.cmp(&a.last_activity))
    });
    conv_usages.truncate(20);
    agg.top_conversations = conv_usages;

    if transcripts_found > 0 {
        agg.sources_checked.push(format!(
            "antigravity-cli/brain/*/transcript.jsonl ({transcripts_found} files)"
        ));
    }
    if conv_dbs_found > 0 {
        agg.sources_checked.push(format!(
            "antigravity-cli/conversations/*.db ({conv_dbs_found} DBs)"
        ));
    }

    agg
}

fn add_step_to_window(
    w: &mut TokenWindowStats,
    in_tok: u64,
    cache_tok: u64,
    out_tok: u64,
    think_tok: u64,
    resp_tok: u64,
    is_error: bool,
) {
    w.input_tokens += in_tok;
    w.cache_read_tokens += cache_tok;
    w.output_tokens += out_tok;
    w.thinking_tokens += think_tok;
    w.response_tokens += resp_tok;
    w.fresh_total_tokens += in_tok + out_tok;
    w.combined_total_tokens += in_tok + cache_tok + out_tok;
    w.planner_steps += 1;
    if is_error {
        w.error_steps += 1;
    }
}

fn load_conversation_metadata(antigravity_dir: &Path) -> HashMap<String, ConvMeta> {
    let mut map = HashMap::new();

    // Primary: cache/conversation_metadata.json
    let json_path = antigravity_dir
        .join("cache")
        .join("conversation_metadata.json");
    if let Ok(raw) = fs::read_to_string(&json_path) {
        if let Ok(root) = serde_json::from_str::<Value>(&raw) {
            if let Some(convs) = root.get("conversations").and_then(|v| v.as_object()) {
                for (cid, entry) in convs {
                    let summary = entry.get("summary").unwrap_or(entry);
                    let title = summary
                        .get("Title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let workspace = summary
                        .get("WorkspaceURIs")
                        .and_then(|v| v.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    map.insert(cid.clone(), ConvMeta { title, workspace });
                }
            }
        }
    }

    // Supplement from conversation_summaries.db if available
    let db_path = antigravity_dir.join("conversation_summaries.db");
    if db_path.is_file() {
        if let Ok(conn) = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
            if let Ok(mut stmt) = conn.prepare(
                "SELECT conversation_id, title, workspace_uris FROM conversation_summaries",
            ) {
                if let Ok(rows) = stmt.query_map([], |row| {
                    let cid: String = row.get(0)?;
                    let title: String = row.get(1)?;
                    let ws: String = row.get(2)?;
                    Ok((cid, title, ws))
                }) {
                    for (cid, title, ws) in rows.flatten() {
                        let first_ws = if let Ok(arr) = serde_json::from_str::<Vec<String>>(&ws) {
                            arr.into_iter().next().unwrap_or_default()
                        } else {
                            ws.lines().next().unwrap_or("").trim().to_string()
                        };
                        map.entry(cid).or_insert(ConvMeta {
                            title,
                            workspace: first_ws,
                        });
                    }
                }
            }
        }
    }

    map
}

fn read_gen_metadata(db_path: &Path) -> Vec<GenStepMeta> {
    let conn = match Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let step_timestamps = read_step_timestamps(&conn);

    let Ok(mut stmt) = conn.prepare("SELECT idx, data FROM gen_metadata ORDER BY idx ASC") else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| {
        let idx: i64 = row.get(0)?;
        let data: Vec<u8> = row.get(1)?;
        Ok((idx, data))
    }) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (_idx, data) in rows.flatten() {
        let mut meta = decode_gen_meta_blob(&data);
        if meta.has_data() {
            let ord = out.len();
            if let Some(&ts) = step_timestamps.get(ord) {
                meta.created_at = ts;
            }
            out.push(meta);
        }
    }
    out
}

fn read_step_timestamps(conn: &Connection) -> Vec<Option<DateTime<Utc>>> {
    let Ok(mut stmt) =
        conn.prepare("SELECT metadata FROM steps WHERE step_type = 15 ORDER BY idx ASC")
    else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| {
        let data: Option<Vec<u8>> = row.get(0).ok();
        Ok(data)
    }) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for blob_opt in rows.flatten() {
        let ts = blob_opt.as_deref().and_then(decode_step_timestamp_blob);
        out.push(ts);
    }
    out
}

fn decode_step_timestamp_blob(buf: &[u8]) -> Option<DateTime<Utc>> {
    for (fn_top, wt_top, val_top) in pb_fields(buf) {
        if matches!(fn_top, 1 | 6 | 7 | 8) && wt_top == 2 {
            let mut secs: Option<i64> = None;
            let mut nanos: u32 = 0;
            for (fn_ts, wt_ts, val_ts) in pb_fields(val_top) {
                if wt_ts == 0 {
                    if let Some(v) = decode_varint_full(val_ts) {
                        match fn_ts {
                            1 => secs = i64::try_from(v).ok(),
                            2 => nanos = u32::try_from(v).unwrap_or(0),
                            _ => {}
                        }
                    }
                }
            }
            if let Some(s) = secs.filter(|&s| s > 1_600_000_000) {
                if let Some(dt) = DateTime::from_timestamp(s, nanos) {
                    return Some(dt);
                }
            }
        }
    }
    None
}

pub fn decode_gen_meta_blob(buf: &[u8]) -> GenStepMeta {
    let mut meta = GenStepMeta::default();
    let mut fallback_slug: Option<String> = None;

    for (fn_top, wt_top, val_top) in pb_fields(buf) {
        if fn_top == 1 && wt_top == 2 {
            for (fn_inner, wt_inner, val_inner) in pb_fields(val_top) {
                match (fn_inner, wt_inner) {
                    (21, 2) => {
                        if let Ok(s) = std::str::from_utf8(val_inner) {
                            if !s.trim().is_empty() {
                                meta.model_label = Some(s.trim().to_string());
                            }
                        }
                    }
                    (19, 2) => {
                        if let Ok(s) = std::str::from_utf8(val_inner) {
                            if !s.trim().is_empty() {
                                fallback_slug = Some(s.trim().to_string());
                            }
                        }
                    }
                    (4, 2) => {
                        for (fn_stat, wt_stat, val_stat) in pb_fields(val_inner) {
                            if wt_stat == 0 {
                                if let Some(v) = decode_varint_full(val_stat) {
                                    match fn_stat {
                                        2 => meta.input_tokens = v,
                                        3 => meta.output_tokens = v,
                                        5 => meta.cache_read_tokens = v,
                                        9 => meta.thinking_tokens = v,
                                        10 => meta.response_tokens = v,
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if meta.output_tokens == 0 && (meta.thinking_tokens > 0 || meta.response_tokens > 0) {
        meta.output_tokens = meta.thinking_tokens + meta.response_tokens;
    }

    if meta.model_label.is_none() {
        meta.model_label = fallback_slug;
    }
    meta
}

fn pb_fields(buf: &[u8]) -> Vec<(u32, u8, &[u8])> {
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < buf.len() {
        let Some((key, next_i)) = read_varint(buf, i) else {
            break;
        };
        i = next_i;
        let field_num = (key >> 3) as u32;
        let wire_type = (key & 0x07) as u8;
        match wire_type {
            0 => {
                let start = i;
                let Some((_, end_i)) = read_varint(buf, i) else {
                    break;
                };
                out.push((field_num, wire_type, &buf[start..end_i]));
                i = end_i;
            }
            1 => {
                if i + 8 > buf.len() {
                    break;
                }
                i += 8;
            }
            2 => {
                let Some((len, start)) = read_varint(buf, i) else {
                    break;
                };
                let len = len as usize;
                if start
                    .checked_add(len)
                    .map(|e| e > buf.len())
                    .unwrap_or(true)
                {
                    break;
                }
                out.push((field_num, wire_type, &buf[start..start + len]));
                i = start + len;
            }
            5 => {
                if i + 4 > buf.len() {
                    break;
                }
                i += 4;
            }
            _ => break,
        }
    }
    out
}

fn read_varint(buf: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let mut val = 0u64;
    let mut shift = 0u32;
    while i < buf.len() && shift < 64 {
        let b = buf[i];
        i += 1;
        val |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some((val, i));
        }
        shift += 7;
    }
    None
}

fn decode_varint_full(slice: &[u8]) -> Option<u64> {
    read_varint(slice, 0).map(|(v, _)| v)
}

pub fn iso_week_for_day(day: &str) -> Option<(String, NaiveDate)> {
    let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    let iw = date.iso_week();
    Some((format!("{:04}-W{:02}", iw.year(), iw.week()), date))
}
