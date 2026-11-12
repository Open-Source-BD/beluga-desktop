//! Which files are secret: the model may not read or change them without the
//! user's say-so. Decided from the file name alone.

const TEMPLATES: [&str; 3] = [".env.example", ".env.sample", ".env.template"];

/// Whether the file at `shown` (a forward-slash path; a backslash is also
/// treated as a separator) is a secret file. Case-insensitive.
pub fn is_secret(shown: &str) -> bool {
    let name = shown
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(shown)
        .to_lowercase();
    if name == ".env" || name.starts_with(".env.") {
        return !TEMPLATES.contains(&name.as_str());
    }
    name.ends_with(".pem")
        || name.ends_with(".key")
        || name.starts_with("id_rsa")
        || name.starts_with("id_ed25519")
        || name.starts_with("credentials")
        || matches!(name.as_str(), ".npmrc" | ".netrc" | ".pypirc")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_pattern_is_secret() {
        for p in [
            ".env",
            ".env.local",
            ".env.production",
            "config/.env",
            "a/b/server.pem",
            "x.key",
            "id_rsa",
            "id_rsa.pub",
            "id_ed25519",
            "id_ed25519.pub",
            ".npmrc",
            ".netrc",
            ".pypirc",
            "credentials",
            "credentials.json",
            "aws/credentials",
        ] {
            assert!(is_secret(p), "{p} should be secret");
        }
    }

    #[test]
    fn the_three_templates_are_not_secret() {
        for p in [
            ".env.example",
            ".env.sample",
            ".env.template",
            "app/.env.example",
        ] {
            assert!(!is_secret(p), "{p} should not be secret");
        }
    }

    #[test]
    fn matching_ignores_case() {
        for p in [
            ".ENV",
            ".Env.Local",
            "KEY.PEM",
            "Id_RSA",
            "ID_ED25519.PUB",
            ".NPMRC",
            "Credentials.JSON",
        ] {
            assert!(is_secret(p), "{p} should be secret");
        }
        assert!(!is_secret(".ENV.EXAMPLE"));
    }

    #[test]
    fn ordinary_files_and_folder_names_are_not_secret() {
        for p in [
            "environment.ts",
            "src/env.ts",
            "keyboard.ts",
            "README.md",
            "pem/readme.txt",
            ".env/readme.txt",
            "monkey",
            "src/credentials/index.ts",
        ] {
            assert!(!is_secret(p), "{p} should not be secret");
        }
    }

    #[test]
    fn only_the_file_name_counts_even_with_backslashes() {
        assert!(is_secret("a\\b\\.env"));
        assert!(!is_secret(""));
    }
}
