//! Syntax highlighting fixes on top of gpui-kit's built-in grammars.
//!
//! The grammars themselves are picked by `tree-sitter-<lang>` features in the workspace
//! `Cargo.toml`. Three of them ship incomplete highlight queries in gpui-kit 0.10, so their
//! code would come out (nearly) uncolored; [`init`] registers complete ones:
//!
//! - C++: upstream's C++ query only adds to the C one; both are needed.
//! - C#: gpui-kit registers the grammar with an empty query.
//! - TSX: needs the JavaScript and JSX queries under the TypeScript one.
//!
//! Query order follows each grammar's own `tree-sitter.json`. When two patterns capture the
//! same node, the first one wins.

use gpui_kit::component::highlighter::{GrammarConfig, LanguageRegistry};

pub fn init() {
    let registry = LanguageRegistry::singleton();

    let cpp = [
        tree_sitter_cpp::HIGHLIGHT_QUERY,
        tree_sitter_c::HIGHLIGHT_QUERY,
    ]
    .join("\n");
    registry.register(
        "cpp",
        &GrammarConfig::new(
            "cpp",
            tree_sitter_cpp::LANGUAGE.into(),
            vec![],
            &cpp,
            "",
            "",
        ),
    );

    registry.register(
        "csharp",
        &GrammarConfig::new(
            "csharp",
            tree_sitter_c_sharp::LANGUAGE.into(),
            vec![],
            tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
    );

    let tsx = [
        tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_typescript::HIGHLIGHTS_QUERY,
    ]
    .join("\n");
    registry.register(
        "tsx",
        &GrammarConfig::new(
            "tsx",
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            vec![],
            &tsx,
            "",
            tree_sitter_typescript::LOCALS_QUERY,
        ),
    );
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    use gpui_kit::HighlightStyle;
    use gpui_kit::base::input::HighlightStyleResolver;
    use gpui_kit::component::highlighter::SyntaxHighlighter;
    use gpui_kit::component::input::Rope;

    /// Records the highlight names a highlighter asks the theme for.
    struct Names(Mutex<BTreeSet<String>>);

    impl HighlightStyleResolver for Names {
        fn style(&self, name: &str) -> Option<HighlightStyle> {
            self.0.lock().unwrap().insert(name.to_owned());
            None
        }
    }

    fn names(language: &str, source: &str) -> BTreeSet<String> {
        let mut highlighter = SyntaxHighlighter::new(language);
        highlighter.update(None, &Rope::from(source), None);
        let names = Names(Mutex::new(BTreeSet::new()));
        let _ = highlighter.styles(&(0..source.len()), &names);
        names.0.into_inner().unwrap()
    }

    /// Every compiled-in language colors its code, with the names chat code blocks need
    /// (aliases included, as the agent writes them after the opening fence).
    #[test]
    fn compiled_in_languages_are_colored() {
        super::init();
        let cases: &[(&str, &str, &[&str])] = &[
            (
                "json",
                r#"{"a": "b", "n": 1, "t": true}"#,
                &["property", "string", "number"],
            ),
            (
                "md",
                "# Title\n\nSome **bold** and `code`.\n",
                &["title", "emphasis.strong"],
            ),
            (
                "py",
                "def f(x):\n    return \"s\"  # c\n",
                &["keyword", "function", "string", "comment"],
            ),
            (
                "rs",
                "fn f() -> u32 { let s = \"x\"; 1 } // c\n",
                &["keyword", "function", "string", "comment"],
            ),
            (
                "js",
                "function f(a) { return 'x'; } // c\n",
                &["keyword", "function", "string", "comment"],
            ),
            (
                "ts",
                "function f(a: number): string { return 'x'; }\n",
                &["keyword", "function", "string", "type"],
            ),
            (
                "tsx",
                "function A() { return <div className=\"x\">{'y'}</div>; } // c\n",
                &["keyword", "function", "string", "comment", "tag"],
            ),
            (
                "c",
                "int main(void) { printf(\"x\"); return 0; } // c\n",
                &["keyword", "function", "string", "comment"],
            ),
            (
                "c++",
                "#include <vector>\nint main() { auto s = \"x\"; f(); return 0; } // c\n",
                &["keyword", "function", "string", "comment"],
            ),
            (
                "cs",
                "class A { void F() { var s = \"x\"; G(); } } // c\n",
                &["keyword", "function", "string", "comment"],
            ),
            (
                "html",
                "<div class=\"x\"><!-- c --></div>\n",
                &["tag", "attribute", "string", "comment"],
            ),
            (
                "css",
                ".a { color: #fff; margin: 1px; } /* c */\n",
                &["property", "comment"],
            ),
            (
                "bash",
                "for f in *; do echo \"$f\"; done # c\n",
                &["keyword", "string", "comment"],
            ),
            (
                "yaml",
                "name: x\nok: true # c\n",
                &["property", "boolean", "comment"],
            ),
            (
                "toml",
                "[package]\nname = \"x\" # c\n",
                &["property", "string", "comment"],
            ),
            (
                "go",
                "package main\nfunc f() string { return \"x\" } // c\n",
                &["keyword", "string", "comment"],
            ),
            (
                "java",
                "class A { void f() { g(\"x\"); } } // c\n",
                &["keyword", "string", "comment"],
            ),
            (
                "diff",
                "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n",
                &["keyword", "string"],
            ),
        ];
        for (language, source, expected) in cases {
            let found = names(language, source);
            for name in *expected {
                // A name like `function.method` counts as `function`, as the theme falls back.
                let ok = found
                    .iter()
                    .any(|found| found == name || found.starts_with(&format!("{name}.")));
                assert!(ok, "{language}: no `{name}` highlight, got {found:?}");
            }
        }
    }
}
