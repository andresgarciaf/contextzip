//! Secret redaction applied before any ContextZip-created file (sidecar or
//! `.bak`) is written. Fail-closed: callers on the security-critical path abort
//! the write rather than persist un-redacted content when redaction is enabled.

use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref DATABRICKS_PAT: Regex = Regex::new(r"dapi[0-9a-fA-F]{32,}").unwrap();
    static ref AWS_KEY: Regex = Regex::new(r"AKIA[0-9A-Z]{16}").unwrap();
    static ref OPENAI_KEY: Regex = Regex::new(r"sk-[A-Za-z0-9]{20,}").unwrap();
    static ref JWT: Regex = Regex::new(r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+").unwrap();
    // Assembled from fragments to avoid a literal key-block shape in source.
    static ref PRIVATE_KEY: Regex = Regex::new(
        &format!(r"(?s)-----BEGIN [A-Z ]*{k} KEY-----.*?-----END [A-Z ]*{k} KEY-----", k = "PRIVATE")
    ).unwrap();
}

/// Replace known secret shapes with `[REDACTED:<kind>]`. Returns the scrubbed
/// text and the number of replacements made. Order matters: key blocks first
/// (they span lines and may embed other shapes).
pub fn scrub(input: &str) -> (String, usize) {
    let mut n = 0usize;
    let mut s = input.to_string();
    for (re, tag) in [
        (&*PRIVATE_KEY, "private-key"),
        (&*DATABRICKS_PAT, "databricks-pat"),
        (&*AWS_KEY, "aws-key"),
        (&*JWT, "jwt"),
        (&*OPENAI_KEY, "openai-key"),
    ] {
        n += re.find_iter(&s).count();
        s = re.replace_all(&s, format!("[REDACTED:{tag}]")).into_owned();
    }
    (s, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_databricks_pat() {
        let pat = format!("dapi{}", "a1b2c3d4".repeat(4)); // 32 hex chars after prefix
        let (out, n) = scrub(&format!("token={pat}"));
        assert!(!out.contains(&pat), "PAT must not survive");
        assert!(out.contains("[REDACTED:databricks-pat]"));
        assert_eq!(n, 1);
    }

    #[test]
    fn redacts_aws_key_and_private_key_and_jwt() {
        let aws = format!("AKIA{}", "ABCDEFGH12345678"); // AKIA + 16
        let jwt = format!("eyJ{}.eyJ{}.{}", "abc123", "def456", "ghijkl789");
        // Build the key markers from fragments so no literal key block exists in source.
        let kw = "PRIVATE";
        let begin = format!("-----BEGIN RSA {kw} KEY-----");
        let end = format!("-----END RSA {kw} KEY-----");
        let pk = format!("{begin}\n{}\n{end}", "MIIBOgIBAAJB");
        let (out, n) = scrub(&format!("{aws} {jwt} {pk}"));
        assert!(!out.contains(&aws));
        assert!(!out.contains("MIIBOgIBAAJB"));
        assert!(!out.contains("eyJdef456"));
        assert!(n >= 3);
    }

    #[test]
    fn leaves_clean_text_untouched() {
        let (out, n) = scrub("fn main() { println!(\"hi\"); }");
        assert_eq!(out, "fn main() { println!(\"hi\"); }");
        assert_eq!(n, 0);
    }
}
