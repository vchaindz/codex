use super::*;
use pretty_assertions::assert_eq;
use unicode_width::UnicodeWidthStr;

fn text(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

fn render(source: &str, width: u16) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    parse(source).expect("panel").lines(width, &mut out);
    out
}

#[test]
fn styled_text_keeps_its_colours_lines_and_links_without_escapes() {
    let lines = render(
        r#"{"type":"text","children":[{"type":"text","props":{"color":"red","weight":"bold"},"children":["● FAILED"]}," lint\n✓ ok ",{"type":"link","props":{"href":"http://127.0.0.1:1/"},"children":["web"]}]}"#,
        40,
    );
    assert_eq!(text(&lines), ["● FAILED lint", "✓ ok web"]);
    assert!(
        lines[0].spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert_ne!(lines[0].spans[0].style, lines[0].spans[1].style);
}

#[test]
fn a_panel_frames_its_widgets_and_offers_each_key_once() {
    let source = r#"{"type":"panel","children":[
        {"type":"box","title":"Limits","color":"cyan","children":[
            {"type":"gauge","label":"ctx","ratio":0.5},
            {"type":"bars","bars":[{"label":"t1","value":3,"text":"3s"},{"label":"t2","value":6}]},
            {"type":"sparkline","data":[1,4,2,8],"height":2},
            {"type":"chart","series":[{"name":"model","points":[[1,2],[2,5],[3,1]]}],"height":5}
        ]},
        {"type":"action","key":"i","label":"investigate","prompt":"look"},
        {"type":"action","key":"i","label":"again","tool":"dismiss"},
        {"type":"action","key":"x","label":"dismiss","tool":"dismiss","arguments":{"key":"a"}}
    ]}"#;
    let root = parse(source).expect("panel");
    let mut lines = Vec::new();
    root.lines(30, &mut lines);
    let shown = text(&lines);
    assert!(
        shown[0].starts_with("╭─ Limits ─") && shown[0].ends_with('╮'),
        "{shown:?}"
    );
    assert!(
        shown
            .iter()
            .all(|line| UnicodeWidthStr::width(line.as_str()) <= 30),
        "{shown:?}"
    );
    assert_eq!(shown.len(), 1 + 1 + 2 + 2 + 5 + 1 + 3);
    assert!(
        shown[1].contains("ctx") && shown[1].contains('━'),
        "{shown:?}"
    );
    assert_eq!(shown.last().map(String::as_str), Some(" x  dismiss"));
    assert_eq!(
        root.actions(),
        [
            PanelAction {
                key: 'i',
                prompt: Some("look".into()),
                tool: None,
                arguments: None
            },
            PanelAction {
                key: 'x',
                prompt: None,
                tool: Some("dismiss".into()),
                arguments: Some(serde_json::json!({"key": "a"})),
            },
        ]
    );
}

#[test]
fn an_oversized_or_unknown_panel_is_refused() {
    assert!(
        parse(&format!(
            "{{\"type\":\"text\",\"children\":[\"{}\"]}}",
            "x".repeat(MAX_SOURCE_BYTES)
        ))
        .is_err()
    );
    assert!(parse(r#"{"type":"gauge","label":"a","ratio":1,"extra":1}"#).is_err());
    assert!(parse(r#"{"type":"video","src":"x"}"#).is_err());
}

#[test]
fn reader_keys_stay_with_the_reader_and_text_keeps_the_status_line_limits() {
    let root = parse(
        r#"{"type":"panel","children":[
            {"type":"action","key":"j","label":"scroll","tool":"t"},
            {"type":"action","key":"/","label":"find","tool":"t"},
            {"type":"action","key":"i","label":"look","prompt":"p"}]}"#,
    )
    .expect("panel");
    assert_eq!(
        root.actions().iter().map(|a| a.key).collect::<String>(),
        "i"
    );
    let deep = (0..20).fold(r#""x""#.to_string(), |inner, _| {
        format!(r#"{{"type":"bold","children":[{inner}]}}"#)
    });
    assert!(parse(&format!(r#"{{"type":"panel","children":[{deep}]}}"#)).is_err());
    let lines = render(
        &format!(r#"{{"type":"text","children":["{}"]}}"#, "\\n".repeat(4000)),
        20,
    );
    assert!(lines.len() <= usize::from(MAX_TEXT_ROWS));
}

#[test]
fn a_wide_box_title_keeps_the_border_inside_the_panel() {
    let lines = render(r#"{"type":"box","title":"界界界界","children":[]}"#, 20);
    assert!(
        lines.iter().all(|line| line.width() <= 20),
        "{:?}",
        text(&lines)
    );
}
