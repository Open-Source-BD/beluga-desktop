//! The tool definitions sent to the model on every request. Built once, in a
//! fixed order, so the request prefix stays identical and can be cached.

use serde_json::{Value, json};
use std::sync::OnceLock;

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
            },
        },
    })
}

fn build() -> Value {
    let path = |what: &str| json!({"type": "string", "description": what});
    Value::Array(vec![
        tool(
            "read_file",
            "Read a text file as numbered lines. Paths are relative to the project folder. \
             At most 400 lines or 16,000 characters per call; the result says when more remain, \
             so call again with offset. Read a file before you edit it.",
            json!({
                "path": path("File path, relative to the project folder."),
                "offset": {"type": "integer", "description": "First line to read, counting from 1."},
                "limit": {"type": "integer", "description": "Number of lines to read, at most 400."},
            }),
            &["path"],
        ),
        tool(
            "list_dir",
            "List the files and folders in one directory, with file sizes. \
             Leave path out to list the project folder.",
            json!({"path": path("Folder path, relative to the project folder.")}),
            &[],
        ),
        tool(
            "find_files",
            "Find files whose path matches a glob such as **/*.ts. Matching uses the path \
             below the folder searched. Files ignored by .gitignore are left out. \
             Returns at most 200 paths.",
            json!({
                "pattern": path("Glob pattern, for example src/**/*.rs."),
                "path": path("Folder to search, relative to the project folder. Default: the whole project."),
            }),
            &["pattern"],
        ),
        tool(
            "search",
            "Search file contents with a regular expression (Rust syntax, case-sensitive). \
             Files ignored by .gitignore and binary files are left out. \
             Returns at most 100 matching lines with path and line number.",
            json!({
                "pattern": path("Regular expression, at most 1,000 characters."),
                "path": path("Folder or file to search, relative to the project folder. Default: the whole project."),
                "glob": path("Only search files whose path matches this glob, for example *.rs."),
            }),
            &["pattern"],
        ),
        tool(
            "edit_file",
            "Replace exact text in a file. You must have read the file first, and again after \
             it changes. old_text must match exactly once, including whitespace, unless \
             replace_all is true. The user approves each change.",
            json!({
                "path": path("File path, relative to the project folder."),
                "old_text": path("The exact text to replace."),
                "new_text": path("The text to put in its place."),
                "replace_all": {"type": "boolean", "description": "Replace every occurrence instead of exactly one."},
            }),
            &["path", "old_text", "new_text"],
        ),
        tool(
            "write_file",
            "Create a file, or replace a whole file, with the given content. Missing parent \
             folders are created. To replace an existing file you must have read it first. \
             The user approves each change. Prefer edit_file for small changes.",
            json!({
                "path": path("File path, relative to the project folder."),
                "content": path("The complete new content of the file."),
            }),
            &["path", "content"],
        ),
        tool(
            "run_command",
            "Run one shell command in the project folder and return its exit code and output. \
             The command gets no keyboard input, so it must not ask questions. The user \
             approves each command. Set background for servers and other long-running commands.",
            json!({
                "command": path("The complete command line."),
                "timeout_seconds": {"type": "integer", "description": "Seconds before the command is stopped. Default 120, at most 600."},
                "background": {"type": "boolean", "description": "Start it and return at once; read its output later with command_output."},
            }),
            &["command"],
        ),
        tool(
            "update_plan",
            "Replace your checklist for this task. Use it for work with several steps, and \
             update it as steps finish. At most 30 items, each at most 200 characters. \
             The user sees the list.",
            json!({
                "items": {
                    "type": "array",
                    "description": "The whole checklist, in order.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "text": {"type": "string", "description": "What the step is."},
                            "state": {"type": "string", "enum": ["pending", "in_progress", "done"]},
                        },
                        "required": ["text", "state"],
                    },
                },
            }),
            &["items"],
        ),
        tool(
            "command_output",
            "Get the new output and the status of a command you started in the background. \
             Each call returns only output you have not seen yet.",
            json!({"id": path("The id returned when the command was started.")}),
            &["id"],
        ),
        tool(
            "stop_command",
            "Stop a command you started in the background.",
            json!({"id": path("The id returned when the command was started.")}),
            &["id"],
        ),
    ])
}

/// The ten tool definitions, OpenAI style. Built once; every call returns the
/// same value.
pub fn tools() -> Value {
    static TOOLS: OnceLock<Value> = OnceLock::new();
    TOOLS.get_or_init(build).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    const NAMES: [&str; 10] = [
        "read_file",
        "list_dir",
        "find_files",
        "search",
        "edit_file",
        "write_file",
        "run_command",
        "update_plan",
        "command_output",
        "stop_command",
    ];
    const PARAMS: [&str; 14] = [
        "path",
        "offset",
        "limit",
        "pattern",
        "glob",
        "old_text",
        "new_text",
        "replace_all",
        "content",
        "command",
        "timeout_seconds",
        "background",
        "items",
        "id",
    ];

    fn list() -> Vec<Value> {
        tools().as_array().unwrap().clone()
    }

    #[test]
    fn ten_tools_with_the_exact_names_in_order() {
        let names: Vec<_> = list()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, NAMES);
        assert!(list().iter().all(|t| t["type"] == "function"));
    }

    #[test]
    fn every_definition_is_a_json_schema_object_with_known_parameters() {
        for t in list() {
            let f = &t["function"];
            let name = f["name"].as_str().unwrap();
            let p = &f["parameters"];
            assert_eq!(p["type"], "object", "{name}");
            let props = p["properties"].as_object().unwrap();
            for (k, v) in props {
                assert!(PARAMS.contains(&k.as_str()), "{name}.{k}");
                assert!(v["type"].is_string(), "{name}.{k}");
                let d = v["description"].as_str().unwrap_or_default();
                assert!(
                    !d.is_empty() && d.split_whitespace().count() < 60,
                    "{name}.{k}"
                );
            }
            for r in p["required"].as_array().unwrap() {
                assert!(
                    props.contains_key(r.as_str().unwrap()),
                    "{name} requires {r}"
                );
            }
            let words = f["description"]
                .as_str()
                .unwrap()
                .split_whitespace()
                .count();
            assert!((5..60).contains(&words), "{name}: {words} words");
        }
    }

    #[test]
    fn required_arguments_match_the_spec() {
        let required = |n: &str| -> Vec<String> {
            let t = list()
                .into_iter()
                .find(|t| t["function"]["name"] == n)
                .unwrap();
            t["function"]["parameters"]["required"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(required("read_file"), ["path"]);
        assert!(required("list_dir").is_empty());
        assert_eq!(required("find_files"), ["pattern"]);
        assert_eq!(required("search"), ["pattern"]);
        assert_eq!(required("edit_file"), ["path", "old_text", "new_text"]);
        assert_eq!(required("write_file"), ["path", "content"]);
        assert_eq!(required("run_command"), ["command"]);
        assert_eq!(required("update_plan"), ["items"]);
        assert_eq!(required("command_output"), ["id"]);
        assert_eq!(required("stop_command"), ["id"]);
    }

    #[test]
    fn plan_items_are_objects_with_a_state_enum() {
        let t = list()
            .into_iter()
            .find(|t| t["function"]["name"] == "update_plan")
            .unwrap();
        let item = &t["function"]["parameters"]["properties"]["items"]["items"];
        assert_eq!(
            item["properties"]["state"]["enum"],
            serde_json::json!(["pending", "in_progress", "done"])
        );
        assert_eq!(item["required"], serde_json::json!(["text", "state"]));
    }

    #[test]
    fn the_value_is_identical_on_every_call() {
        assert_eq!(tools().to_string(), tools().to_string());
    }
}
