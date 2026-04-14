use legend_pure_parser_pure::model::PureModel;

/// Load platform, extracting the model regardless of errors.
fn load_platform_model() -> (
    PureModel,
    Vec<legend_pure_parser_pure::error::CompilationError>,
) {
    match legend_pure_core_platform::platform::load_platform() {
        Ok(model) => (model, vec![]),
        Err(partial) => (partial.model, partial.errors),
    }
}

#[test]
fn platform_parse_recovery() {
    // Verify that error recovery works by checking element counts from partial parses.
    let platform_sources = legend_pure_core_platform::sources::platform_sources();
    let total = platform_sources.len();
    let mut clean = 0;
    let mut partial = 0;
    let mut total_errors = 0;
    let mut total_elements = 0;

    for s in platform_sources {
        match legend_pure_parser_parser::parse(s.content, s.path) {
            Ok(ast) => {
                total_elements += ast.element_count();
                clean += 1;
            }
            Err(p) => {
                total_elements += p.source_file.element_count();
                total_errors += p.errors.len();
                partial += 1;
                for e in &p.errors {
                    eprintln!("  PARTIAL: {} -> {}", s.path, e);
                }
            }
        }
    }
    eprintln!("\nPlatform parse summary:");
    eprintln!("  Total files: {total}");
    eprintln!("  Clean parses: {clean}");
    eprintln!("  Partial parses: {partial}");
    eprintln!("  Total elements recovered: {total_elements}");
    eprintln!("  Total errors: {total_errors}");

    assert_eq!(clean + partial, total, "every file should produce a result");
    assert!(
        total_elements > 300,
        "Expected >300 recovered elements, got {total_elements}"
    );
}

#[test]
fn plus_registered_in_model() {
    let (model, errors) = load_platform_model();
    eprintln!("Platform compilation errors: {}", errors.len());

    // Check math package has plus elements
    let root = model.root_package;
    let root_pkg = model.get_package(root);
    let meta_id = root_pkg
        .children_packages
        .iter()
        .find(|&&id| model.global_packages.get(id.0).name == "meta")
        .copied()
        .expect("meta");
    let pure_id = model
        .get_package(meta_id)
        .children_packages
        .iter()
        .find(|&&id| model.global_packages.get(id.0).name == "pure")
        .copied()
        .expect("pure");
    let funcs_id = model
        .get_package(pure_id)
        .children_packages
        .iter()
        .find(|&&id| model.global_packages.get(id.0).name == "functions")
        .copied()
        .expect("functions");
    let math_id = model
        .get_package(funcs_id)
        .children_packages
        .iter()
        .find(|&&id| model.global_packages.get(id.0).name == "math")
        .copied()
        .expect("math");

    let math_pkg = model.get_package(math_id);
    eprintln!("math children: {}", math_pkg.children_elements.len());

    let plus_elements: Vec<_> = math_pkg
        .children_elements
        .iter()
        .filter(|&&eid| model.get_node(eid).name.contains("plus"))
        .collect();
    eprintln!(
        "plus elements in math: {:?}",
        plus_elements
            .iter()
            .map(|&&eid| model.get_node(eid).name.as_str())
            .collect::<Vec<_>>()
    );

    assert!(
        !plus_elements.is_empty(),
        "plus should be registered in math package"
    );
}
