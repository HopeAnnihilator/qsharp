// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{path::PathBuf, sync::Arc};

use expect_test::Expect;
use qsc_project::{FileSystem, Manifest, Project, StdFs};

pub fn check(project_path: &PathBuf, expect: &Expect) {
    let mut root_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root_path.push(PathBuf::from("tests/projects"));
    let mut absolute_project_path = root_path.clone();
    absolute_project_path.push(project_path);
    let manifest = Manifest::load_from_path(absolute_project_path)
        .expect("manifest should load")
        .expect("manifest should contain descriptor");
    let fs = StdFs;
    let mut project = fs
        .load_project_with_deps(&manifest.manifest_dir, None)
        .expect("project should load")
        .package_graph_sources
        .root;

    // remove the prefix absolute path
    for (path, _contents) in &mut project.sources {
        let new_path = PathBuf::from(path.to_string());
        let new_path = new_path
            .strip_prefix(&root_path)
            .expect("prefix should be present")
            .to_string_lossy();
        let new_path = new_path.replace(std::path::MAIN_SEPARATOR, "/");
        *path = Arc::from(new_path);
    }

    project.sources.sort();

    let project = Project {
        sources: project.sources,
        manifest: manifest.manifest,
    };

    expect.assert_eq(&format!("{project:#?}"));
}
