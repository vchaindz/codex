//! A sidebar provider's panel: the status line's styled text, native charts and keys that act on it.

use crate::history_cell::HistoryCell;
use crate::terminal_hyperlinks::HyperlinkLine;
use crate::ui_tree::UiNode;
use crate::ui_tree::UiTree;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::layout::Size;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::symbols::Marker;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Axis;
use ratatui::widgets::Bar;
use ratatui::widgets::BarChart;
use ratatui::widgets::Chart;
use ratatui::widgets::Dataset;
use ratatui::widgets::GraphType;
use ratatui::widgets::LineGauge;
use ratatui::widgets::Sparkline;
use ratatui::widgets::Widget;
use ratatui::widgets::canvas::Canvas;
use ratatui::widgets::canvas::Points;
use serde::Deserialize;
use std::str::FromStr;

#[cfg(test)]
#[path = "sidebar_panel_tests.rs"]
mod tests;

// serde_json's arbitrary_precision makes a float a map once a tagged enum buffers it; `Number` reads that back.
use serde_json::Number;
const MAX_SOURCE_BYTES: usize = 64 * 1024;
const MAX_HEIGHT: u16 = 24;
const MAX_TEXT_ROWS: u16 = 512;
// Keys the document reader already uses: a provider cannot take them.
const RESERVED_KEYS: &str = "hjklgGqvyonNdubf";

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum Block {
    Panel {
        children: Vec<Block>,
    },
    Box {
        #[serde(default)]
        title: String,
        #[serde(default)]
        color: Option<String>,
        children: Vec<Block>,
    },
    Gauge {
        label: String,
        ratio: Number,
        #[serde(default)]
        color: Option<String>,
    },
    Sparkline {
        data: Vec<u64>,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        height: Option<u16>,
    },
    Bars {
        bars: Vec<BarSpec>,
    },
    Chart {
        series: Vec<Series>,
        #[serde(default)]
        height: Option<u16>,
    },
    Pie {
        slices: Vec<BarSpec>,
        #[serde(default)]
        height: Option<u16>,
    },
    Action {
        key: char,
        label: String,
        #[serde(default)]
        prompt: Option<String>,
        #[serde(default)]
        tool: Option<String>,
        #[serde(default)]
        arguments: Option<serde_json::Value>,
    },
    #[serde(untagged)]
    Text(UiNode),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BarSpec {
    label: String,
    value: u64,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    color: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Series {
    name: String,
    points: Vec<(Number, Number)>,
    #[serde(default)]
    color: Option<String>,
}

/// A key the panel offers: it drafts `prompt` in the composer and calls `tool` on the provider's server.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PanelAction {
    pub(super) key: char,
    pub(super) prompt: Option<String>,
    pub(super) tool: Option<String>,
    pub(super) arguments: Option<serde_json::Value>,
}

pub(super) fn parse(source: &str) -> Result<Block, String> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err("Sidebar panel exceeds 64 KiB".into());
    }
    let root: Block = serde_json::from_str(source)
        .map_err(|error| format!("Unreadable sidebar panel: {error}"))?;
    root.check_text()?;
    Ok(root)
}

fn color(name: &Option<String>, fallback: Color) -> Color {
    name.as_deref()
        .and_then(|name| Color::from_str(name).ok())
        .unwrap_or(fallback)
}

impl Block {
    /// Styled text keeps the status line's limits, which the panel's own decoding would skip.
    fn check_text(&self) -> Result<(), String> {
        match self {
            Self::Panel { children } | Self::Box { children, .. } => {
                children.iter().try_for_each(Self::check_text)
            }
            Self::Text(node) => serde_json::to_string(node)
                .map_err(|error| error.to_string())?
                .parse::<UiTree>()
                .map(drop)
                .map_err(|error| format!("Unreadable sidebar panel text: {error}")),
            _ => Ok(()),
        }
    }

    pub(super) fn actions(&self) -> Vec<PanelAction> {
        let mut found = Vec::new();
        self.collect_actions(&mut found);
        found
    }

    fn collect_actions(&self, found: &mut Vec<PanelAction>) {
        match self {
            Self::Panel { children } | Self::Box { children, .. } => {
                children
                    .iter()
                    .for_each(|child| child.collect_actions(found));
            }
            Self::Action {
                key,
                prompt,
                tool,
                arguments,
                ..
            } if key.is_ascii_alphanumeric()
                && !RESERVED_KEYS.contains(*key)
                && !found.iter().any(|action| action.key == *key) =>
            {
                found.push(PanelAction {
                    key: *key,
                    prompt: prompt.clone(),
                    tool: tool.clone(),
                    arguments: arguments.clone(),
                })
            }
            _ => {}
        }
    }

    fn lines(&self, width: u16, out: &mut Vec<HyperlinkLine>) {
        if width == 0 {
            return;
        }
        let tall = |height: Option<u16>, default| height.unwrap_or(default).clamp(1, MAX_HEIGHT);
        match self {
            Self::Panel { children } => children.iter().for_each(|child| child.lines(width, out)),
            Self::Box {
                title,
                color: name,
                children,
            } => {
                let mut inner = Vec::new();
                let room = width.saturating_sub(4);
                children
                    .iter()
                    .for_each(|child| child.lines(room, &mut inner));
                if room == 0 {
                    out.extend(inner);
                    return;
                }
                let edge = Style::default().fg(color(name, Color::DarkGray));
                let title =
                    crate::line_truncation::document_label(&[title.as_str()], usize::from(room));
                let label = if title.is_empty() {
                    String::new()
                } else {
                    format!(" {title} ")
                };
                let rule = usize::from(width - 2)
                    .saturating_sub(unicode_width::UnicodeWidthStr::width(label.as_str()) + 1);
                out.push(HyperlinkLine::new(Line::from(vec![
                    Span::styled("╭─", edge),
                    Span::styled(label, edge.add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{}╮", "─".repeat(rule)), edge),
                ])));
                for mut row in inner {
                    let pad = usize::from(room).saturating_sub(row.width());
                    let mut spans = vec![Span::styled("│ ", edge)];
                    spans.extend(row.line.spans);
                    spans.push(Span::styled(format!("{} │", " ".repeat(pad)), edge));
                    row.line = Line::from(spans);
                    row.source = None;
                    for link in &mut row.hyperlinks {
                        link.columns = link.columns.start + 2..link.columns.end + 2;
                    }
                    out.push(row);
                }
                out.push(HyperlinkLine::new(Line::styled(
                    format!("╰{}╯", "─".repeat(usize::from(width - 2))),
                    edge,
                )));
            }
            Self::Gauge {
                label,
                ratio,
                color: name,
            } => out.extend(draw(
                LineGauge::default()
                    .ratio(
                        ratio
                            .as_f64()
                            .filter(|r| r.is_finite())
                            .map_or(0.0, |r| r.clamp(0.0, 1.0)),
                    )
                    .label(label.clone())
                    .filled_symbol("━")
                    .filled_style(Style::default().fg(color(name, Color::Green)))
                    .unfilled_style(Style::default().fg(Color::DarkGray)),
                width,
                1,
            )),
            Self::Sparkline {
                data,
                color: name,
                height,
            } => {
                let shown = &data[data.len().saturating_sub(usize::from(width))..];
                out.extend(draw(
                    Sparkline::default()
                        .data(shown)
                        .style(Style::default().fg(color(name, Color::Cyan))),
                    width,
                    tall(*height, 3),
                ));
            }
            Self::Bars { bars } => {
                let shown = &bars[..bars.len().min(usize::from(MAX_HEIGHT))];
                let bars: Vec<Bar> = shown
                    .iter()
                    .map(|bar| {
                        let spec = Bar::with_label(bar.label.clone(), bar.value)
                            .style(Style::default().fg(color(&bar.color, Color::Cyan)));
                        match &bar.text {
                            Some(text) => spec.text_value(text.clone()),
                            None => spec,
                        }
                    })
                    .collect();
                out.extend(draw(
                    BarChart::horizontal(bars).bar_width(1).bar_gap(0),
                    width,
                    shown.len() as u16,
                ));
            }
            Self::Chart { series, height } => {
                let series: Vec<(&Series, Vec<(f64, f64)>)> = series
                    .iter()
                    .map(|s| {
                        let points = s
                            .points
                            .iter()
                            .filter_map(|(x, y)| Some((x.as_f64()?, y.as_f64()?)));
                        (
                            s,
                            points
                                .filter(|(x, y)| x.is_finite() && y.is_finite())
                                .collect(),
                        )
                    })
                    .collect();
                let points = || series.iter().flat_map(|(_, points)| points.iter().copied());
                let (low_x, high_x) = points().fold((f64::MAX, f64::MIN), |(lo, hi), (x, _)| {
                    (lo.min(x), hi.max(x))
                });
                let high_y = points().fold(0.0_f64, |hi, (_, y)| hi.max(y));
                if low_x > high_x {
                    return;
                }
                let datasets = series
                    .iter()
                    .map(|(s, points)| {
                        Dataset::default()
                            .name(s.name.clone())
                            .marker(Marker::Braille)
                            .graph_type(GraphType::Line)
                            .style(Style::default().fg(color(&s.color, Color::Cyan)))
                            .data(points)
                    })
                    .collect();
                let top = if high_y > 0.0 { high_y * 1.1 } else { 1.0 };
                let axis = Style::default().fg(Color::DarkGray);
                out.extend(draw(
                    Chart::new(datasets)
                        .x_axis(
                            Axis::default()
                                .style(axis)
                                .bounds([low_x, high_x.max(low_x + 1.0)]),
                        )
                        .y_axis(
                            Axis::default()
                                .style(axis)
                                .bounds([0.0, top])
                                .labels(["0".to_string(), format!("{high_y:.0}")]),
                        ),
                    width,
                    tall(*height, 8),
                ));
            }
            Self::Pie { slices, height } => pie(slices, width, tall(*height, 6), out),
            Self::Action { key, label, .. } => out.push(HyperlinkLine::new(Line::from(vec![
                Span::styled(
                    format!(" {key} "),
                    Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
                ),
                Span::raw(format!(" {label}")),
            ]))),
            Self::Text(node) => {
                let tree = UiTree(Some(node.clone())).prepare();
                let size = tree.measure(Size::new(width, MAX_TEXT_ROWS));
                let area = Rect::new(0, 0, size.width, size.height);
                let mut buf = Buffer::empty(area);
                crate::render::highlight::with_syntax_theme(|theme| {
                    tree.render(area, theme, &mut buf, /*plain_style*/ None);
                });
                out.extend(buffer_lines(&buf));
            }
        }
    }
}

/// A disc of `height` rows, its slices clockwise from the top, with each slice's share beside it.
fn pie(slices: &[BarSpec], width: u16, height: u16, out: &mut Vec<HyperlinkLine>) {
    let slices = &slices[..slices.len().min(usize::from(MAX_HEIGHT))];
    let total: u64 = slices.iter().map(|slice| slice.value).sum();
    if total == 0 {
        return;
    }
    // A braille dot is about as wide as it is tall, so a disc is twice as many cells across as down.
    let across = (height * 2).min(width);
    let (dots_x, dots_y) = (usize::from(across) * 2, usize::from(height) * 4);
    let mut points: Vec<Vec<(f64, f64)>> = vec![Vec::new(); slices.len()];
    for i in 0..dots_x {
        for j in 0..dots_y {
            let x = (i as f64 + 0.5) / dots_x as f64 * 2.0 - 1.0;
            let y = (j as f64 + 0.5) / dots_y as f64 * 2.0 - 1.0;
            if x * x + y * y > 1.0 {
                continue;
            }
            let turn = x.atan2(y).rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU;
            let mut upto = 0;
            let slice = slices
                .iter()
                .position(|slice| {
                    upto += slice.value;
                    turn < upto as f64 / total as f64
                })
                .unwrap_or(slices.len() - 1);
            points[slice].push((x, y));
        }
    }
    let mut rows = draw(
        Canvas::default()
            .marker(Marker::Braille)
            .x_bounds([-1.0, 1.0])
            .y_bounds([-1.0, 1.0])
            .paint(|ctx| {
                for (slice, coords) in slices.iter().zip(&points) {
                    ctx.draw(&Points {
                        coords,
                        color: color(&slice.color, Color::Cyan),
                    });
                }
            }),
        across,
        height,
    );
    rows.resize_with(rows.len().max(slices.len()), HyperlinkLine::default);
    for (row, slice) in rows.iter_mut().zip(slices) {
        let pad = usize::from(across).saturating_sub(row.width());
        let share = slice.value as f64 / total as f64 * 100.0;
        let text = slice.text.clone().unwrap_or_else(|| format!("{share:.0}%"));
        row.line.spans.extend([
            Span::raw(" ".repeat(pad + 2)),
            Span::styled("● ", Style::default().fg(color(&slice.color, Color::Cyan))),
            Span::raw(format!("{} {text}", slice.label)),
        ]);
    }
    out.extend(rows);
}

fn draw(widget: impl Widget, width: u16, height: u16) -> Vec<HyperlinkLine> {
    let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
    widget.render(buf.area, &mut buf);
    buffer_lines(&buf)
}

fn buffer_lines(buf: &Buffer) -> Vec<HyperlinkLine> {
    let area = buf.area;
    (area.top()..area.bottom())
        .map(|y| {
            // Runs of cells with one style and one link.
            let mut runs: Vec<(Span<'static>, Option<String>)> = Vec::new();
            let mut x = area.left();
            while x < area.right() {
                let cell = &buf[(x, y)];
                let style = cell.style();
                let raw = cell.symbol();
                // A link cell wraps its text in OSC 8: `ESC ]8;;url BEL text ESC ]8;; BEL`.
                let link = raw
                    .strip_prefix("\x1b]8;;")
                    .and_then(|rest| rest.split_once('\x07'))
                    .map(|(url, _)| url.to_owned())
                    .filter(|url| !url.is_empty());
                let symbol = crate::ui_tree::sanitize_leaf(raw.to_owned());
                // A wide symbol covers the cells after it.
                x += unicode_width::UnicodeWidthStr::width(symbol.as_str()).max(1) as u16;
                match runs.last_mut() {
                    Some((span, href)) if span.style == style && *href == link => {
                        span.content.to_mut().push_str(&symbol)
                    }
                    _ => runs.push((Span::styled(symbol, style), link)),
                }
            }
            // A buffer is as wide as its widest line; shorter lines end in unpainted cells.
            if let Some((last, None)) = runs.last_mut()
                && last.style.bg.is_none_or(|bg| bg == Color::Reset)
            {
                let end = last.content.trim_end().len();
                last.content.to_mut().truncate(end);
            }
            let mut row = HyperlinkLine::default();
            for (span, link) in runs {
                row.push_span(span, link.as_deref());
            }
            row
        })
        .collect()
}

#[derive(Debug)]
pub(super) struct PanelCell {
    pub(super) source: String,
    pub(super) root: Block,
}

impl HistoryCell for PanelCell {
    fn copy_source(&self) -> Option<&str> {
        Some(&self.source)
    }

    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.display_hyperlink_lines(width)
            .into_iter()
            .map(|row| row.line)
            .collect()
    }

    fn display_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        let mut out = Vec::new();
        self.root.lines(width, &mut out);
        out
    }

    fn transcript_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        self.display_hyperlink_lines(width)
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        crate::history_cell::raw_lines_from_source(&self.source)
    }
}
