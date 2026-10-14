//! Escaping values placed into the listen page.

/// Escapes text for interpolation into HTML.
///
/// The forwarded target reaches this page as a raw query parameter and is placed into an `href`
/// and into the page body. Its *host* is checked against an allowlist, but nothing checks its path
/// or query — so `?u=https://youtube.com/">…` passes the host test carrying whatever it likes.
/// Unescaped, that is script execution on this origin, which is where the dashboard keeps its
/// device token.
pub(crate) fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

/// JSON destined for a `<script>` block.
///
/// A JSON string may legally contain `</script>`, which ends the block from the parser's point of
/// view no matter what the JSON says. Escaping the slash keeps the value identical to JavaScript
/// while making that sequence impossible.
pub(super) fn escape_script_json(value: &str) -> String {
    value.replace("</", "<\\/")
}
