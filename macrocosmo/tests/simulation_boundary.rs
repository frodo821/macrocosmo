use std::fs;
use std::path::{Path, PathBuf};

const FORBIDDEN_PATTERNS: &[&str] = &[
    "bevy_egui",
    "egui::",
    "crate::ui",
    "crate::visualization",
    "crate::input",
    "KeyCode",
    "ButtonInput",
    "Window",
];

const CHECK_PATHS: &[&str] = &[
    "src/simulation.rs",
    "src/choice.rs",
    "src/notifications.rs",
    "src/scripting",
    "src/setup",
    "src/player",
    "src/time_system",
    "src/observer",
];

#[test]
fn simulation_boundary_does_not_import_interactions() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut violations = Vec::new();

    for rel in CHECK_PATHS {
        collect_violations(&manifest_dir.join(rel), &manifest_dir, &mut violations);
    }

    assert!(
        violations.is_empty(),
        "simulation-side files must not import UI/input/rendering symbols:\n{}",
        violations.join("\n")
    );
}

fn collect_violations(path: &Path, manifest_dir: &Path, violations: &mut Vec<String>) {
    if path.is_dir() {
        for entry in fs::read_dir(path).expect("read source dir") {
            let entry = entry.expect("read source dir entry");
            collect_violations(&entry.path(), manifest_dir, violations);
        }
        return;
    }

    if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
        return;
    }

    let text = fs::read_to_string(path).expect("read source file");
    for (line_idx, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with("*") {
            continue;
        }
        for pattern in FORBIDDEN_PATTERNS {
            if line.contains(pattern) {
                let rel = path.strip_prefix(manifest_dir).unwrap_or(path);
                violations.push(format!(
                    "{}:{} contains `{}`",
                    rel.display(),
                    line_idx + 1,
                    pattern
                ));
            }
        }
    }
}
