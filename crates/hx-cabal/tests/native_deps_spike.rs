//! Phase-1 spike for native dependency builds.
//!
//! Drives the (previously unwired) `FullNativeBuilder` end to end on one real
//! pure-Haskell dependency (`split`, which depends only on `base`): resolve →
//! fetch from Hackage → build the dependency from source → register it → build
//! a local project against it → run the result.
//!
//! Ignored by default: needs network access and a working GHC on PATH.
//! Run with: `cargo test -p hx-cabal --test native_deps_spike -- --ignored --nocapture`

use std::path::PathBuf;

use hx_cabal::native::GhcConfig;
use hx_cabal::{FullNativeBuildOptions, FullNativeBuilder, pre_installed_packages};
use hx_solver::{
    FetchOptions, InstallPlan, PlanOptions, ResolvedPackage, fetch_packages, generate_build_plan,
};
use hx_ui::Output;

#[tokio::test]
#[ignore = "needs network + GHC; run with --ignored"]
async fn full_native_builds_one_real_dependency() {
    let output = Output::new();

    // Resolve GHC.
    let ghc_path = which::which("ghc").expect("ghc on PATH");
    let ghc = GhcConfig::detect_with_path(&ghc_path)
        .await
        .expect("detect GHC");
    eprintln!("GHC {} at {}", ghc.version, ghc.ghc_path.display());

    // A minimal install plan: one pure-Haskell package whose only dependency is
    // base (so no transitive source builds are needed for this first spike).
    let mut plan = InstallPlan::new();
    plan.add(ResolvedPackage {
        name: "split".to_string(),
        version: "0.2.5".parse().expect("version"),
        dependencies: vec!["base".to_string()],
    });
    // A boot package the local project also uses, to exercise `-package <name>`
    // exposure of pre-installed packages GHC hides by default.
    plan.add(ResolvedPackage {
        name: "containers".to_string(),
        version: "0.6.8".parse().expect("version"),
        dependencies: vec!["base".to_string()],
    });

    // Fetch the dependency tarball(s) from Hackage.
    let fetched = fetch_packages(&plan, &FetchOptions::default())
        .await
        .expect("fetch split");
    eprintln!(
        "fetched: {:?}",
        fetched.iter().map(|f| &f.name).collect::<Vec<_>>()
    );

    // Topologically ordered build plan; base is pre-installed and must not build.
    let plan_opts = PlanOptions {
        compiler_id: format!("ghc-{}", ghc.version),
        platform: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        pre_installed: pre_installed_packages()
            .into_iter()
            .map(str::to_string)
            .collect(),
        cached_hashes: Default::default(),
    };
    let build_plan = generate_build_plan(&plan, &plan_opts).expect("build plan");

    // A local project that imports the dependency.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/Main.hs"),
        [
            "module Main where\n",
            "import Data.List.Split (splitOn)\n",
            "import qualified Data.Map as M\n",
            "main :: IO ()\n",
            "main = do\n",
            "  putStrLn (unwords (splitOn \",\" \"a,b,c\"))\n",
            "  print (M.size (M.fromList [(1::Int,'a'),(2,'b')]))\n",
        ]
        .concat(),
    )
    .unwrap();

    // Build everything natively.
    let cache_dir =
        hx_solver::default_package_cache_dir().unwrap_or_else(|| PathBuf::from(".hx/native-store"));
    let mut builder = FullNativeBuilder::new(ghc, cache_dir)
        .await
        .expect("create builder");
    let opts = FullNativeBuildOptions {
        skip_unsupported: false, // we want a hard failure if split can't build
        verbose: true,
        ..Default::default()
    };
    let local_options = hx_cabal::native::NativeBuildOptions {
        src_dirs: vec![PathBuf::from("src")],
        output_dir: root.join(".hx/native-build"),
        main_module: Some("Main".to_string()),
        output_exe: Some(root.join(".hx/native-build/app")),
        native_linking: true,
        ..Default::default()
    };
    let result = builder
        .build_project(root, &build_plan, &fetched, &opts, &local_options, &output)
        .await
        .expect("build_project call");

    eprintln!(
        "result: success={} built={} skipped={} failed={} registered={:?} errors={:?}",
        result.success,
        result.packages_built,
        result.packages_skipped,
        result.packages_failed,
        result.registered_packages,
        result.errors
    );

    // The dependency must have been built and registered, and the local project
    // must have compiled against it.
    assert!(
        result.success,
        "full native build failed: {:?}",
        result.errors
    );
    assert!(
        result.packages_built >= 1,
        "split was not built from source"
    );
    assert!(
        result
            .registered_packages
            .iter()
            .any(|p| p.contains("split")),
        "split was not registered: {:?}",
        result.registered_packages
    );
    let proj = result.project_result.expect("project result");
    assert!(proj.success, "local project did not build");

    // Run the produced executable and check its output.
    if let Some(exe) = proj.executable {
        let out = std::process::Command::new(&exe).output().expect("run exe");
        let stdout = String::from_utf8_lossy(&out.stdout);
        eprintln!("exe output: {stdout:?}");
        assert!(
            stdout.contains("a b c") && stdout.contains("2"),
            "unexpected output: {stdout:?}"
        );
    }
}
