use super::*;

/// The payload that motivated this: the host passes the allowlist, the path carries the break.
#[test]
fn a_target_cannot_break_out_of_its_href() {
    let hostile = "https://youtube.com/\"><script>alert(1)</script>";
    let escaped = escape_html(hostile);
    assert!(!escaped.contains('<'));
    assert!(!escaped.contains('"'));
    assert!(escaped.starts_with("https://youtube.com/&quot;&gt;&lt;script&gt;"));
}

#[test]
fn ordinary_urls_survive_readably() {
    let ordinary = "https://music.youtube.com/watch?v=dQw4w9WgXcQ";
    assert_eq!(escape_html(ordinary), ordinary);
}

#[test]
fn json_cannot_close_the_script_block() {
    let hostile = r#""https://x.test/</script><script>alert(1)</script>""#;
    assert!(!escape_script_json(hostile).contains("</script>"));
}
