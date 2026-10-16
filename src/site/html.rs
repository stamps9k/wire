//! HTML output helpers: escaping untrusted text and filling templates.
//!
//! The two are kept separate. [`render`] inserts values exactly as given, so
//! anything that came from a request must go through [`escape`] first.

use crate::site::render_error::RenderError;

/// Escapes `text` so it can be placed in HTML as text.
///
/// Replaces `&`, `<`, `>`, `"` and `'` with `&amp;`, `&lt;`, `&gt;`,
/// `&quot;` and `&#39;`. Every other character, including multi-byte UTF-8,
/// is copied unchanged. `&` is replaced first, so the `&` in the other
/// replacements is never escaped again.
///
/// The result is safe between tags and inside an attribute value quoted with
/// `"` or `'`. It is not safe inside `<script>` or `<style>`, in an unquoted
/// attribute, or as a URL in `href` or `src`, where `javascript:` needs none
/// of these characters.
pub fn escape(text: &str) -> String {
  text
    .replace('&', "&amp;")
    .replace('<', "&lt;")
    .replace('>', "&gt;")
    .replace('"', "&quot;")
    .replace('\'', "&#39;")
}

/// Fills the `{{name}}` placeholders in `template` with `values`.
///
/// The template is scanned once, from left to right. Text outside
/// placeholders is copied unchanged, including a lone `{`, `}` or `}}`. Each
/// `{{` starts a placeholder that ends at the next `}}`, and the text between
/// them is the name. Names are matched exactly: they are not trimmed and are
/// case-sensitive. A name may appear more than once, values that are not
/// used are ignored, and the order of `values` does not matter.
///
/// Values are inserted verbatim. They are not escaped, so a value can
/// contain markup, and they are never scanned for placeholders, so a `{{` in
/// a value is plain text. Escaping untrusted text with [`escape`] is the
/// caller's job.
///
/// # Errors
///
/// Returns [`RenderError::Unclosed`] if a `{{` has no `}}` before the end of
/// the template or before the next `{{`. Returns
/// [`RenderError::UnknownName`] if a name, including the empty name in
/// `{{}}`, is not one of the keys in `values`.
pub fn render(
  template: &str,
  values: &[(&str, &str)],
) -> Result<String, RenderError> {
  let mut out = String::with_capacity(template.len());

  let mut pieces = template.split("{{");

  out.push_str(pieces.next().unwrap_or(""));

  for chunk in pieces {
    let Some((name, text)) = chunk.split_once("}}") else {
      return Err(RenderError::Unclosed);
    };

    let Some((_, value)) = values.iter().find(|(key, _)| *key == name) else {
      return Err(RenderError::UnknownName(name.to_string()));
    };

    out.push_str(value);
    out.push_str(text);
  }

  Ok(out)
}

#[cfg(test)]
mod tests {
  use super::*;

  // escape

  #[test]
  fn escape_empty_is_empty() {
    assert_eq!(escape(""), "");
  }

  #[test]
  fn escape_leaves_plain_text_unchanged() {
    assert_eq!(escape("hello world"), "hello world");
  }

  #[test]
  fn escape_leaves_other_punctuation_unchanged() {
    let text = "/?=;:#%_-+.,!*()[]{}|\\^~`@$";
    assert_eq!(escape(text), text);
  }

  #[test]
  fn escape_ampersand() {
    assert_eq!(escape("&"), "&amp;");
  }

  #[test]
  fn escape_less_than() {
    assert_eq!(escape("<"), "&lt;");
  }

  #[test]
  fn escape_greater_than() {
    assert_eq!(escape(">"), "&gt;");
  }

  #[test]
  fn escape_double_quote() {
    assert_eq!(escape("\""), "&quot;");
  }

  #[test]
  fn escape_single_quote() {
    assert_eq!(escape("'"), "&#39;");
  }

  #[test]
  fn escape_every_occurrence() {
    assert_eq!(escape("<<>>"), "&lt;&lt;&gt;&gt;");
  }

  #[test]
  fn escape_mixed_text_keeps_order() {
    assert_eq!(
      escape(r#"<a href="x">'&'</a>"#),
      "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
    );
  }

  #[test]
  fn escape_does_not_double_escape_its_own_output() {
    // Each `&` in the input is escaped exactly once, so an
    // entity in the input is shown literally, not decoded.
    assert_eq!(escape("&lt;"), "&amp;lt;");
    assert_eq!(escape("&amp;"), "&amp;amp;");
  }

  #[test]
  fn escape_script_injection() {
    let escaped = escape("<script>alert(1)</script>");
    assert_eq!(escaped, "&lt;script&gt;alert(1)&lt;/script&gt;");
    assert!(!escaped.contains('<'));
    assert!(!escaped.contains('>'));
  }

  #[test]
  fn escape_keeps_multibyte_characters() {
    assert_eq!(escape("日本<語>"), "日本&lt;語&gt;");
  }

  #[test]
  fn escape_keeps_the_replacement_character() {
    assert_eq!(escape("a\u{fffd}b"), "a\u{fffd}b");
  }

  // render

  #[allow(
    clippy::unnecessary_wraps,
    reason = "builds the expected Result for comparison with render"
  )]
  fn ok(text: &str) -> Result<String, RenderError> {
    Ok(text.to_owned())
  }

  fn unknown(name: &str) -> Result<String, RenderError> {
    Err(RenderError::UnknownName(name.to_owned()))
  }

  #[test]
  fn render_empty_template_is_empty() {
    assert_eq!(render("", &[("a", "x")]), ok(""));
  }

  #[test]
  fn render_without_placeholders_is_unchanged() {
    assert_eq!(render("<p>hi</p>", &[("a", "x")]), ok("<p>hi</p>"));
  }

  #[test]
  fn render_without_placeholders_or_values_is_unchanged() {
    assert_eq!(render("<p>hi</p>", &[]), ok("<p>hi</p>"));
  }

  #[test]
  fn render_replaces_a_placeholder() {
    assert_eq!(render("<p>{{a}}</p>", &[("a", "x")]), ok("<p>x</p>"));
  }

  #[test]
  fn render_replaces_several_placeholders() {
    let values = [("a", "1"), ("b", "2"), ("c", "3")];
    assert_eq!(render("{{a}},{{b}},{{c}}", &values), ok("1,2,3"));
  }

  #[test]
  fn render_order_of_values_does_not_matter() {
    let forwards = [("a", "1"), ("b", "2")];
    let backwards = [("b", "2"), ("a", "1")];
    assert_eq!(render("{{a}} {{b}}", &forwards), ok("1 2"));
    assert_eq!(render("{{a}} {{b}}", &backwards), ok("1 2"));
  }

  #[test]
  fn render_replaces_every_occurrence() {
    assert_eq!(render("{{a}} and {{a}}", &[("a", "x")]), ok("x and x"));
  }

  #[test]
  fn render_placeholders_at_start_and_end() {
    let values = [("a", "start"), ("b", "end")];
    assert_eq!(
      render("{{a}} middle {{b}}", &values),
      ok("start middle end")
    );
  }

  #[test]
  fn render_adjacent_placeholders() {
    let values = [("a", "1"), ("b", "2")];
    assert_eq!(render("{{a}}{{b}}", &values), ok("12"));
  }

  #[test]
  fn render_ignores_unused_values() {
    let values = [("a", "1"), ("unused", "2")];
    assert_eq!(render("{{a}}", &values), ok("1"));
  }

  #[test]
  fn render_lone_braces_are_text() {
    let template = "{ } }} {a} a}} }}{";
    assert_eq!(render(template, &[("a", "x")]), ok(template));
  }

  #[test]
  fn render_unknown_name_is_an_error() {
    assert_eq!(render("<p>{{x}}</p>", &[("a", "1")]), unknown("x"));
  }

  #[test]
  fn render_unknown_name_with_no_values_is_an_error() {
    assert_eq!(render("{{a}}", &[]), unknown("a"));
  }

  #[test]
  fn render_empty_name_is_an_error() {
    assert_eq!(render("{{}}", &[("a", "1")]), unknown(""));
  }

  #[test]
  fn render_names_are_not_trimmed() {
    assert_eq!(render("{{ a }}", &[("a", "1")]), unknown(" a "));
  }

  #[test]
  fn render_names_are_case_sensitive() {
    assert_eq!(render("{{A}}", &[("a", "1")]), unknown("A"));
  }

  #[test]
  fn render_unclosed_placeholder_is_an_error() {
    assert_eq!(
      render("<p>{{a</p>", &[("a", "1")]),
      Err(RenderError::Unclosed)
    );
  }

  #[test]
  fn render_open_braces_at_end_is_an_error() {
    assert_eq!(render("text{{", &[("a", "1")]), Err(RenderError::Unclosed));
  }

  #[test]
  fn render_close_before_open_is_still_unclosed() {
    assert_eq!(render("}}{{a", &[("a", "1")]), Err(RenderError::Unclosed));
  }

  #[test]
  fn render_empty_value_removes_the_placeholder() {
    assert_eq!(render("<ul>{{a}}</ul>", &[("a", "")]), ok("<ul></ul>"));
  }

  #[test]
  fn render_inserts_values_verbatim() {
    // Escaping is the caller's job: the headers list is already markup.
    let values = [("a", "<li>a & b</li>")];
    assert_eq!(
      render("<ul>{{a}}</ul>", &values),
      ok("<ul><li>a & b</li></ul>")
    );
  }

  #[test]
  fn render_does_not_expand_a_placeholder_inside_a_value() {
    let values = [("a", "{{b}}"), ("b", "x")];
    assert_eq!(render("{{a}} {{b}}", &values), ok("{{b}} x"));
  }

  #[test]
  fn render_does_not_expand_a_value_containing_its_own_placeholder() {
    assert_eq!(render("{{a}}", &[("a", "[{{a}}]")]), ok("[{{a}}]"));
  }

  #[test]
  fn render_unclosed_braces_inside_a_value_are_not_an_error() {
    assert_eq!(render("{{a}}", &[("a", "{{")]), ok("{{"));
  }

  #[test]
  fn render_unknown_names_inside_a_value_are_not_an_error() {
    assert_eq!(render("{{a}}", &[("a", "{{nope}}")]), ok("{{nope}}"));
  }

  #[test]
  fn render_keeps_multibyte_text_around_placeholders() {
    assert_eq!(render("日本{{a}}語", &[("a", "x")]), ok("日本x語"));
  }

  #[test]
  fn render_inserts_multibyte_values() {
    assert_eq!(
      render("<p>{{a}}</p>", &[("a", "日本語")]),
      ok("<p>日本語</p>")
    );
  }

  #[test]
  fn render_multibyte_names() {
    assert_eq!(render("{{名前}}", &[("名前", "x")]), ok("x"));
  }

  // together

  #[test]
  fn escaped_values_cannot_inject_markup_or_placeholders() {
    let target = escape("<script>{{b}}</script>");
    let values = [("a", target.as_str()), ("b", "x")];
    assert_eq!(
      render("<p>{{a}}</p><p>{{b}}</p>", &values),
      ok("<p>&lt;script&gt;{{b}}&lt;/script&gt;</p><p>x</p>")
    );
  }

  #[test]
  fn render_unclosed_placeholder_before_another_is_an_error() {
    let values = [("a", "1"), ("b", "2")];
    assert_eq!(render("x{{a y{{b}}z", &values), Err(RenderError::Unclosed));
  }

  #[test]
  fn render_doubled_open_braces_is_an_error() {
    assert_eq!(render("{{{{a}}", &[("a", "1")]), Err(RenderError::Unclosed));
  }
}
