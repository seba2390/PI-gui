//! The sites sandboxed commands may reach when the network is limited ([`Network::Custom`]).
//!
//! Over HTTPS only the site a connection goes to can be seen, not the page, so a site is allowed
//! as a whole: `github.com` is that site, `github.com/*` also its subdomains (`api.github.com`).
//! What the user types is checked as they type ([`parse`]): `https://www.google.com/search`
//! becomes `www.google.com` (with a note that pages can't be told apart), `httttp://…` and
//! `wwww.google.com` are refused with what was probably meant.
//!
//! [`Network::Custom`]: crate::Network::Custom

/// An allowed site, as it is saved: `host` or `host/*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// Lowercase, without a trailing dot.
    pub host: String,
    /// Its subdomains too.
    pub subdomains: bool,
}

impl Site {
    /// How it is saved and shown: `github.com` or `github.com/*`.
    pub fn label(&self) -> String {
        if self.subdomains {
            format!("{}/*", self.host)
        } else {
            self.host.clone()
        }
    }

    /// Whether a connection to `host` goes to this site.
    pub fn matches(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        host == self.host
            || (self.subdomains
                && host.len() > self.host.len()
                && host.ends_with(&self.host)
                && host.as_bytes()[host.len() - self.host.len() - 1] == b'.')
    }
}

/// What typing an address gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub site: Site,
    /// Something to know about it (a page that stands for its whole site).
    pub note: Option<String>,
}

/// Reads what the user typed as a site, or says what is wrong with it.
pub fn parse(input: &str) -> Result<Parsed, String> {
    let text = input.trim().to_ascii_lowercase();
    if text.is_empty() {
        return Err("Type an address, like github.com or github.com/*".into());
    }
    if text.contains(char::is_whitespace) {
        return Err("An address has no spaces".into());
    }
    let rest = match text.split_once("://") {
        Some((scheme, rest)) => {
            if scheme != "http" && scheme != "https" {
                let hint = if scheme.starts_with("ht")
                    || scheme.ends_with("tp")
                    || scheme.ends_with("tps")
                {
                    " (did you mean https://?)"
                } else {
                    ""
                };
                return Err(format!("\"{scheme}://\" is not a web address{hint}"));
            }
            rest
        }
        None if text.starts_with("http:") || text.starts_with("https:") => {
            return Err("After http: or https: comes //".into());
        }
        None => text.as_str(),
    };
    let (rest, mut subdomains) = match rest.strip_prefix("*.") {
        Some(rest) => (rest, true),
        None => (rest, false),
    };
    let (authority, path) = match rest.find(['/', '?', '#']) {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    let mut note = None;
    match path.trim_end_matches('/') {
        "" => {}
        "/*" | "*" => subdomains = true,
        page => {
            note = Some(format!(
                "Pages can't be told apart, so this allows all of {} (not only {page})",
                authority.split(':').next().unwrap_or(authority)
            ));
        }
    }
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => {
            if port.parse::<u16>().is_err() {
                return Err(format!("{port} is not a port"));
            }
            host
        }
        Some((_, "")) => return Err("A port comes after the colon".into()),
        Some(_) => return Err("Only a port number can come after a colon".into()),
        None => authority,
    };
    let host = host.trim_end_matches('.');
    check_host(host)?;
    Ok(Parsed {
        site: Site {
            host: host.to_owned(),
            subdomains,
        },
        note,
    })
}

/// Whether `host` is a name a site can have, or says why not.
fn check_host(host: &str) -> Result<(), String> {
    if host.is_empty() {
        return Err("The address has no site name".into());
    }
    if host.parse::<std::net::Ipv4Addr>().is_ok() || host == "localhost" {
        return Ok(());
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 {
        return Err(format!("\"{host}\" needs a domain ending, like {host}.com"));
    }
    for label in &labels {
        if label.is_empty() {
            return Err("Two dots in a row".into());
        }
        if label.len() > 63 {
            return Err("A part of the name is longer than 63 letters".into());
        }
        if let Some(bad) = label
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || *c == '-'))
        {
            return Err(format!("\"{bad}\" can't be in a site name"));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err("A part of the name can't start or end with -".into());
        }
    }
    let ending = labels[labels.len() - 1];
    if ending.len() < 2 || !ending.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(format!("\".{ending}\" is not a domain ending"));
    }
    // `wwww.google.com`, `ww.google.com`: almost always a typo.
    if labels.len() > 2
        && labels[0] != "www"
        && labels[0].len() >= 2
        && labels[0].chars().all(|c| c == 'w')
    {
        let suggestion = format!("www.{}", labels[1..].join("."));
        return Err(format!("Did you mean {suggestion}?"));
    }
    Ok(())
}

/// Whether `host` is one of the saved `sites` (labels as [`Site::label`] writes them).
pub fn allowed(sites: &[String], host: &str) -> bool {
    sites
        .iter()
        .filter_map(|label| parse(label).ok())
        .any(|parsed| parsed.site.matches(host))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(input: &str) -> String {
        parse(input).expect("valid").site.label()
    }

    #[test]
    fn addresses_become_sites() {
        assert_eq!(site("github.com"), "github.com");
        assert_eq!(site("  GitHub.com/ "), "github.com");
        assert_eq!(site("https://github.com"), "github.com");
        assert_eq!(site("github.com/*"), "github.com/*");
        assert_eq!(site("*.github.com"), "github.com/*");
        assert_eq!(site("http://localhost:8080"), "localhost");
        assert_eq!(site("127.0.0.1"), "127.0.0.1");
        assert_eq!(site("registry.npmjs.org:443"), "registry.npmjs.org");
        let page = parse("https://www.google.com/search?q=x").expect("valid");
        assert_eq!(page.site.label(), "www.google.com");
        assert!(page.note.expect("a note").contains("all of www.google.com"));
    }

    #[test]
    fn typos_are_caught_with_what_was_meant() {
        for (input, says) in [
            ("httttp://google.com", "did you mean https://"),
            ("htps://google.com", "did you mean https://"),
            ("ftp://example.com", "not a web address"),
            ("https:/google.com", "comes //"),
            ("wwww.google.com", "Did you mean www.google.com?"),
            ("ww.google.com", "Did you mean www.google.com?"),
            ("google", "needs a domain ending"),
            ("google..com", "Two dots"),
            ("goo gle.com", "no spaces"),
            ("google.c0m", "not a domain ending"),
            ("exa_mple.com", "\"_\" can't be"),
            ("example.com:99999", "not a port"),
            ("", "Type an address"),
        ] {
            let error = parse(input).expect_err(input);
            assert!(error.contains(says), "{input}: {error}");
        }
    }

    #[test]
    fn subdomains_match_only_with_the_star() {
        let sites = ["github.com".to_owned(), "npmjs.org/*".to_owned()];
        assert!(allowed(&sites, "github.com"));
        assert!(allowed(&sites, "GitHub.com."));
        assert!(!allowed(&sites, "api.github.com"));
        assert!(allowed(&sites, "npmjs.org"));
        assert!(allowed(&sites, "registry.npmjs.org"));
        assert!(!allowed(&sites, "evilnpmjs.org"));
        assert!(!allowed(&sites, "npmjs.org.evil.com"));
    }
}
