/// Get language from file extension or special filename.
pub fn get_language(extension: &str, filename: &str) -> String {
    let ext = extension.to_lowercase();
    let lang = match ext.as_str() {
        ".py" | ".pyi" | ".pyx" => "python",
        ".js" | ".jsx" | ".mjs" | ".cjs" => "javascript",
        ".ts" | ".tsx" => "typescript",
        ".go" => "go",
        ".java" => "java",
        ".kt" | ".kts" => "kotlin",
        ".rs" => "rust",
        ".c" | ".h" => "c",
        ".cpp" | ".hpp" | ".cc" | ".cxx" => "cpp",
        ".cs" => "csharp",
        ".rb" => "ruby",
        ".php" => "php",
        ".swift" => "swift",
        ".scala" => "scala",
        ".sh" | ".bash" => "bash",
        ".zsh" => "zsh",
        ".md" => "markdown",
        ".rst" => "restructuredtext",
        ".adoc" => "asciidoc",
        ".txt" => "text",
        ".yaml" | ".yml" => "yaml",
        ".toml" => "toml",
        ".json" => "json",
        ".ini" | ".cfg" => "ini",
        ".html" => "html",
        ".css" => "css",
        ".scss" => "scss",
        ".less" => "less",
        ".vue" => "vue",
        ".svelte" => "svelte",
        ".sql" => "sql",
        ".dockerfile" => "dockerfile",
        ".graphql" => "graphql",
        ".proto" => "protobuf",
        _ => {
            let name = filename.to_lowercase();
            if name == "dockerfile" {
                return "dockerfile".to_string();
            }
            if name == "makefile" {
                return "makefile".to_string();
            }
            if name == "rakefile" {
                return "ruby".to_string();
            }
            if ext.is_empty() && name.ends_with("rc") {
                return "shell".to_string();
            }
            "text"
        }
    };
    lang.to_string()
}
