//! Environment variables that hold keys and tokens, which sandboxed commands don't get. pi
//! itself (outside the sandbox) keeps them for its model requests.

/// Whether a variable named `name` probably holds a secret.
pub fn is_secret(name: &str) -> bool {
    const SUFFIXES: &[&str] = &[
        "_API_KEY",
        "_APIKEY",
        "_TOKEN",
        "_SECRET",
        "_SECRET_KEY",
        "_SECRET_ACCESS_KEY",
        "_PASSWORD",
        "_PASSWD",
        "_CREDENTIALS",
    ];
    const NAMES: &[&str] = &[
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "HF_TOKEN",
        "NPM_TOKEN",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "AZURE_CLIENT_SECRET",
    ];
    let upper = name.to_ascii_uppercase();
    NAMES.contains(&upper.as_str()) || SUFFIXES.iter().any(|suffix| upper.ends_with(suffix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_tokens_are_secret_paths_are_not() {
        for name in [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "ANTHROPIC_OAUTH_TOKEN",
            "GH_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "my_service_password",
        ] {
            assert!(is_secret(name), "{name}");
        }
        for name in [
            "PATH",
            "HOME",
            "SSH_AUTH_SOCK",
            "TOKENIZERS_PARALLELISM",
            "PI_MODEL",
        ] {
            assert!(!is_secret(name), "{name}");
        }
    }
}
