use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

fn main() {
    let out_dir = env::var_os("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("generated_sources.rs");

    // 1. Platform directory from legend-pure Java module
    let platform_dir = PathBuf::from(
        "../../../legend-pure-core/legend-pure-m3-core/src/main/resources/platform/pure",
    );

    let mut generated_code = String::new();
    generated_code.push_str("/// Array containing all embedded platform Pure files\n");
    generated_code.push_str("pub const PLATFORM_FILES: &[PureSourceFile] = &[\n");

    if platform_dir.exists() {
        for entry in WalkDir::new(&platform_dir) {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "pure") {
                let relative_path = path.strip_prefix(&platform_dir).unwrap();
                let relative_path_str = relative_path.to_str().unwrap().replace('\\', "/");

                // Exclusions
                if relative_path_str == "grammar/m3.pure" {
                    continue;
                }

                let absolute_path = fs::canonicalize(path).unwrap();
                let abs_path_str = absolute_path.to_str().unwrap().replace('\\', "/");

                let _ = write!(
                    generated_code,
                    "    PureSourceFile {{\n        path: \"{relative_path_str}\",\n        content: include_str!(\"{abs_path_str}\"),\n    }},\n"
                );

                println!("cargo:rerun-if-changed={abs_path_str}");
            }
        }
    }

    println!("cargo:rerun-if-changed={}", platform_dir.display());

    generated_code.push_str("];\n");

    fs::write(&dest_path, generated_code).unwrap();
}
