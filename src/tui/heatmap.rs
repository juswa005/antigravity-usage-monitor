use crate::models::DailyTokenBucket;
use crate::tui::theme;
use crate::utils::{format_tokens, format_tokens_exact};
use chrono::{Datelike, Duration, Local, NaiveDate};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeatmapMetric {
    FreshTokens,
    CombinedTokens,
    PlannerSteps,
}

impl HeatmapMetric {
    pub fn next(self) -> Self {
        match self {
            Self::FreshTokens => Self::CombinedTokens,
            Self::CombinedTokens => Self::PlannerSteps,
            Self::PlannerSteps => Self::FreshTokens,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::FreshTokens => "Fresh Tokens (In+Out)",
            Self::CombinedTokens => "Combined Tokens (+Cache)",
            Self::PlannerSteps => "Planner Steps",
        }
    }

    pub fn value_of(self, b: &DailyTokenBucket) -> u64 {
        match self {
            Self::FreshTokens => b.input_tokens + b.output_tokens,
            Self::CombinedTokens => b.input_tokens + b.cache_read_tokens + b.output_tokens,
            Self::PlannerSteps => b.planner_steps,
        }
    }

    pub fn format_value(self, v: u64) -> String {
        match self {
            Self::PlannerSteps => format!("{} steps", format_tokens_exact(v)),
            Self::FreshTokens | Self::CombinedTokens => format_tokens(v),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HeatmapCell {
    pub date: NaiveDate,
    pub value: u64,
    pub level: u8,
    pub bucket: Option<DailyTokenBucket>,
}

#[derive(Debug, Clone)]
pub struct HeatmapGrid {
    pub weeks: Vec<[Option<HeatmapCell>; 7]>,
    pub cells_flat: Vec<HeatmapCell>,
    pub total_value: u64,
    pub active_days: usize,
    pub current_streak: usize,
    pub longest_streak: usize,
    pub peak_day: Option<(NaiveDate, u64)>,
}

impl HeatmapGrid {
    pub fn build(
        buckets: &[DailyTokenBucket],
        end_date: NaiveDate,
        num_weeks: usize,
        metric: HeatmapMetric,
    ) -> Self {
        let num_weeks = num_weeks.max(1);
        let mut by_date: HashMap<NaiveDate, &DailyTokenBucket> = HashMap::new();
        for b in buckets {
            if let Ok(d) = NaiveDate::parse_from_str(&b.day_local, "%Y-%m-%d") {
                by_date.insert(d, b);
            }
        }

        // Compute streaks across all recorded history up to end_date
        let mut active_set: Vec<NaiveDate> = by_date
            .iter()
            .filter(|(_, b)| metric.value_of(b) > 0)
            .map(|(d, _)| *d)
            .collect();
        active_set.sort();

        let mut longest_streak = 0usize;
        let mut run = 0usize;
        let mut prev_d: Option<NaiveDate> = None;
        for d in &active_set {
            if prev_d.map(|p| *d - p == Duration::days(1)).unwrap_or(false) {
                run += 1;
            } else {
                run = 1;
            }
            longest_streak = longest_streak.max(run);
            prev_d = Some(*d);
        }

        let mut current_streak = 0usize;
        let mut cursor = end_date;
        // Allow today to still be 0 if user hasn't run a step today yet, counting from yesterday
        if !by_date
            .get(&cursor)
            .map(|b| metric.value_of(b) > 0)
            .unwrap_or(false)
        {
            cursor -= Duration::days(1);
        }
        while by_date
            .get(&cursor)
            .map(|b| metric.value_of(b) > 0)
            .unwrap_or(false)
        {
            current_streak += 1;
            cursor -= Duration::days(1);
        }

        // Align grid columns to ISO weeks (Mon=0 .. Sun=6)
        let end_wd = end_date.weekday().num_days_from_monday() as i64;
        let last_monday = end_date - Duration::days(end_wd);
        let first_monday = last_monday - Duration::weeks((num_weeks - 1) as i64);

        let mut max_val = 0u64;
        let mut total_value = 0u64;
        let mut active_days = 0usize;
        let mut peak_day: Option<(NaiveDate, u64)> = None;

        for w in 0..num_weeks {
            for r in 0..7 {
                let d = first_monday + Duration::days((w * 7 + r) as i64);
                if d > end_date {
                    continue;
                }
                if let Some(b) = by_date.get(&d) {
                    let v = metric.value_of(b);
                    total_value += v;
                    if v > 0 {
                        active_days += 1;
                        max_val = max_val.max(v);
                        if peak_day.map(|(_, pv)| v > pv).unwrap_or(true) {
                            peak_day = Some((d, v));
                        }
                    }
                }
            }
        }

        let mut weeks = Vec::with_capacity(num_weeks);
        let mut cells_flat = Vec::with_capacity(num_weeks * 7);

        for w in 0..num_weeks {
            let mut col: [Option<HeatmapCell>; 7] = std::array::from_fn(|_| None);
            for (r, slot) in col.iter_mut().enumerate() {
                let d = first_monday + Duration::days((w * 7 + r) as i64);
                if d > end_date {
                    continue;
                }
                let bucket = by_date.get(&d).map(|b| (*b).clone());
                let value = bucket.as_ref().map(|b| metric.value_of(b)).unwrap_or(0);
                let level = compute_intensity_level(value, max_val);
                let cell = HeatmapCell {
                    date: d,
                    value,
                    level,
                    bucket,
                };
                cells_flat.push(cell.clone());
                *slot = Some(cell);
            }
            weeks.push(col);
        }

        Self {
            weeks,
            cells_flat,
            total_value,
            active_days,
            current_streak,
            longest_streak,
            peak_day,
        }
    }
}

pub fn compute_intensity_level(value: u64, max_value: u64) -> u8 {
    if value == 0 || max_value == 0 {
        return 0;
    }
    // Sqrt normalization gives expressive GitHub-like step spread across normal & heavy days
    let ratio = (value as f64).sqrt() / (max_value as f64).sqrt();
    if ratio <= 0.25 {
        1
    } else if ratio <= 0.50 {
        2
    } else if ratio <= 0.75 {
        3
    } else {
        4
    }
}

pub fn dot_color(level: u8) -> Color {
    match level {
        0 => Color::Rgb(42, 52, 49),
        1 => Color::Rgb(25, 86, 48),
        2 => Color::Rgb(42, 134, 68),
        3 => Color::Rgb(74, 184, 92),
        _ => Color::Rgb(126, 232, 130),
    }
}

pub fn render_ratatui_heatmap(
    frame: &mut Frame,
    area: Rect,
    buckets: &[DailyTokenBucket],
    metric: HeatmapMetric,
    selected_back_offset: Option<usize>,
    title: &str,
    compact: bool,
) {
    let block = Block::default()
        .title(Span::styled(title, theme::title()))
        .borders(Borders::ALL)
        .border_style(theme::border())
        .style(theme::base());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 16 || inner.height < 8 {
        return;
    }

    let today = Local::now().date_naive();
    let usable_cols = inner.width.saturating_sub(5) as usize;
    let max_weeks = (usable_cols / 2).clamp(4, if compact { 26 } else { 52 });
    let grid = HeatmapGrid::build(buckets, today, max_weeks, metric);

    let selected_date = selected_back_offset
        .and_then(|back| grid.cells_flat.iter().rev().nth(back).map(|c| c.date));

    let mut lines: Vec<Line> = Vec::new();

    // Month header row
    let mut month_spans = vec![Span::raw("    ")];
    let mut last_month = 0u32;
    let mut skip_col = false;
    for col in &grid.weeks {
        if skip_col {
            skip_col = false;
            continue;
        }
        let m_label = col.iter().flatten().next().and_then(|c| {
            let m = c.date.month();
            if m != last_month {
                last_month = m;
                Some(c.date.format("%b").to_string())
            } else {
                None
            }
        });
        if let Some(lbl) = m_label {
            month_spans.push(Span::styled(format!("{lbl:<4}"), theme::muted()));
            skip_col = true;
        } else {
            month_spans.push(Span::raw("  "));
        }
    }
    lines.push(Line::from(month_spans));

    let day_labels = ["Mon", "   ", "Wed", "   ", "Fri", "   ", "Sun"];
    for (row_idx, day_lbl) in day_labels.iter().enumerate() {
        let mut row_spans = vec![Span::styled(format!("{day_lbl} "), theme::muted())];
        for col in &grid.weeks {
            match &col[row_idx] {
                Some(cell) => {
                    let is_sel = selected_date == Some(cell.date);
                    let ch = if cell.level == 0 { "▪" } else { "■" };
                    let mut style = Style::default().fg(dot_color(cell.level));
                    if is_sel {
                        style = style
                            .bg(theme::FG)
                            .fg(theme::BG)
                            .add_modifier(Modifier::BOLD);
                    }
                    row_spans.push(Span::styled(ch, style));
                    row_spans.push(Span::raw(" "));
                }
                None => row_spans.push(Span::raw("  ")),
            }
        }
        lines.push(Line::from(row_spans));
    }

    // Legend + Streak summary line
    let peak_str = grid
        .peak_day
        .map(|(d, v)| format!("{} ({})", d.format("%m-%d"), metric.format_value(v)))
        .unwrap_or_else(|| "None".to_string());

    let mut summary_spans = vec![
        Span::styled("Less ", theme::muted()),
        Span::styled("▪ ", Style::default().fg(dot_color(0))),
        Span::styled("■ ", Style::default().fg(dot_color(1))),
        Span::styled("■ ", Style::default().fg(dot_color(2))),
        Span::styled("■ ", Style::default().fg(dot_color(3))),
        Span::styled("■ ", Style::default().fg(dot_color(4))),
        Span::styled("More", theme::muted()),
        Span::raw("  │ "),
        Span::styled(format!("Active: {}d", grid.active_days), theme::ok_style()),
        Span::raw(" │ "),
        Span::styled(
            format!(
                "Streak: {}d (max {}d)",
                grid.current_streak, grid.longest_streak
            ),
            Style::default().fg(theme::FG),
        ),
    ];
    if !compact {
        summary_spans.push(Span::raw(" │ "));
        summary_spans.push(Span::styled(format!("Peak: {peak_str}"), theme::muted()));
    }

    if let Some(sel_date) = selected_date {
        if let Some(cell) = grid.cells_flat.iter().find(|c| c.date == sel_date) {
            let detail = match &cell.bucket {
                Some(b) => format!(
                    "{} ({}) │ Fresh: {} │ In: {} │ Out: {} │ Cache: {} │ Steps: {} │ Convs: {}",
                    cell.date.format("%Y-%m-%d"),
                    cell.date.format("%a"),
                    format_tokens(b.input_tokens + b.output_tokens),
                    format_tokens(b.input_tokens),
                    format_tokens(b.output_tokens),
                    format_tokens(b.cache_read_tokens),
                    format_tokens_exact(b.planner_steps),
                    b.conversations_active
                ),
                None => format!(
                    "{} ({}) │ No Antigravity usage recorded",
                    cell.date.format("%Y-%m-%d"),
                    cell.date.format("%a")
                ),
            };
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(8),
                    Constraint::Length(1),
                    Constraint::Min(1),
                ])
                .split(inner);
            frame.render_widget(Paragraph::new(lines), chunks[0]);
            frame.render_widget(Paragraph::new(Line::from(summary_spans)), chunks[1]);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        "Day: ",
                        Style::default()
                            .fg(theme::BRIGHT_TEAL)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(detail),
                    Span::styled("  ([/]: move · [G]: metric)", theme::muted()),
                ])),
                chunks[2],
            );
            return;
        }
    }

    lines.push(Line::from(summary_spans));
    frame.render_widget(Paragraph::new(lines), inner);
}

pub fn render_ansi_heatmap(
    buckets: &[DailyTokenBucket],
    num_weeks: usize,
    metric: HeatmapMetric,
) -> String {
    let today = Local::now().date_naive();
    let grid = HeatmapGrid::build(buckets, today, num_weeks, metric);
    let mut out = String::new();

    out.push_str(&format!(
        "ACTIVITY HEATMAP — {} (last {} weeks | Active: {}d | Streak: {}d | Max: {}d)\n",
        metric.label(),
        num_weeks,
        grid.active_days,
        grid.current_streak,
        grid.longest_streak
    ));

    let ansi_for = |level: u8| -> &'static str {
        match level {
            0 => "\x1b[38;2;52;64;60m▪\x1b[0m",
            1 => "\x1b[38;2;25;86;48m■\x1b[0m",
            2 => "\x1b[38;2;42;134;68m■\x1b[0m",
            3 => "\x1b[38;2;74;184;92m■\x1b[0m",
            _ => "\x1b[38;2;126;232;130m■\x1b[0m",
        }
    };

    let day_labels = ["Mon", "   ", "Wed", "   ", "Fri", "   ", "Sun"];
    for (row_idx, lbl) in day_labels.iter().enumerate() {
        out.push_str(lbl);
        out.push(' ');
        for col in &grid.weeks {
            match &col[row_idx] {
                Some(c) => {
                    out.push_str(ansi_for(c.level));
                    out.push(' ');
                }
                None => out.push_str("  "),
            }
        }
        out.push('\n');
    }
    out.push_str(&format!(
        "    Less {} {} {} {} {} More\n",
        ansi_for(0),
        ansi_for(1),
        ansi_for(2),
        ansi_for(3),
        ansi_for(4)
    ));
    out
}
