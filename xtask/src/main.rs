use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::Command;

const RUST_REPO: &str = "https://github.com/blueshift-gg/rust";
const RUST_BRANCH: &str = "BPF_i128_ret";
const LLVM_REPO: &str = "https://github.com/blueshift-gg/llvm-project.git";
const LINKER_REPO: &str = "https://github.com/blueshift-gg/sbpf-linker";
const LINKER_BRANCH: &str = "u128_mul_libcall";
const TOOLCHAIN_NAME: &str = "stage1";

/// xtask for setting up custom Rust compiler with i128 BPF support
#[derive(Parser)]
#[command(name = "xtask")]
#[command(about = "Build automation for u128 BPF prototype")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Set up the complete toolchain (rust compiler + sbpf linker)
    Setup,
    /// Clone and build the SBPF linker only
    BuildLinker,
    /// Set up and build the Rust compiler with modified LLVM only
    BuildCompiler,
    /// Build the example project with the custom toolchain
    Build,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let project_root = project_root()?;

    match cli.command {
        Commands::Setup => {
            setup_linker(&project_root)?;
            setup_compiler(&project_root)?;
            println!();
            println!("==========================================");
            println!("Setup complete!");
            println!();
            println!("Build this project with:");
            println!("  cargo xtask build");
            println!("  # or directly:");
            println!("  cargo +{} build-bpf", TOOLCHAIN_NAME);
            println!("==========================================");
        }
        Commands::BuildLinker => {
            setup_linker(&project_root)?;
        }
        Commands::BuildCompiler => {
            setup_compiler(&project_root)?;
        }
        Commands::Build => {
            build_project(&project_root)?;
        }
    }

    Ok(())
}

fn project_root() -> Result<PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap());

    // If we're in xtask dir, go up one level
    if manifest_dir.ends_with("xtask") {
        Ok(manifest_dir.parent().unwrap().to_path_buf())
    } else {
        Ok(manifest_dir)
    }
}

fn cache_dir() -> PathBuf {
    // Build tools outside the project to avoid Cargo workspace issues
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("u128-bpf-toolchain")
}

fn setup_linker(project_root: &Path) -> Result<()> {
    let base_dir = cache_dir();
    let linker_dir = base_dir.join("sbpf-linker");
    let linker_bin = linker_dir.join("target/release/sbpf-linker");

    println!("  SBPF linker will be built in: {}", linker_dir.display());

    // Ensure cache directory exists
    std::fs::create_dir_all(&base_dir)?;

    // 1. Clone SBPF linker if needed
    println!("[1/3] Cloning SBPF linker...");
    if linker_dir.exists() {
        println!("  sbpf-linker directory already exists, skipping clone");
    } else {
        run_command(
            Command::new("git")
                .args(["clone", "--branch", LINKER_BRANCH, LINKER_REPO])
                .arg(&linker_dir),
            "clone sbpf-linker",
        )?;
    }

    // 2. Build SBPF linker
    println!("[2/3] Building SBPF linker...");
    run_command(
        Command::new("cargo")
            .args(["build", "--release"])
            .current_dir(&linker_dir),
        "build sbpf-linker",
    )?;

    // 3. Update .cargo/config.toml with linker path
    println!("[3/3] Updating .cargo/config.toml with linker path...");
    let cargo_config_dir = project_root.join(".cargo");
    std::fs::create_dir_all(&cargo_config_dir)?;

    let config_content = format!(
        r#"[unstable]
build-std = ["core", "alloc"]

[target.bpfel-unknown-none]
rustflags = [
    "-C", "linker={}",
    "-C", "panic=abort",
    "-C", "link-arg=--dump-module=llvm_dump",
    "-C", "link-arg=--llvm-args=-bpf-stack-size=4096",
    "-C", "relocation-model=static",
]

[alias]
build-bpf = "build --release --target bpfel-unknown-none"
xtask = "run --package xtask --"
"#,
        linker_bin.display()
    );

    std::fs::write(cargo_config_dir.join("config.toml"), config_content)
        .context("failed to write .cargo/config.toml")?;

    println!("  SBPF linker ready at: {}", linker_bin.display());
    Ok(())
}

fn setup_compiler(_project_root: &Path) -> Result<()> {
    let base_dir = cache_dir();
    let rust_dir = base_dir.join("rust-compiler");
    println!("  Rust compiler will be built in: {}", rust_dir.display());

    // Ensure cache directory exists
    std::fs::create_dir_all(&base_dir)?;

    // 1. Clone Rust compiler if needed
    println!("[1/5] Cloning Rust compiler...");
    if rust_dir.exists() {
        println!("  rust-compiler directory already exists, skipping clone");
    } else {
        run_command(
            Command::new("git")
                .args(["clone", "--branch", RUST_BRANCH, RUST_REPO])
                .arg(&rust_dir),
            "clone rust compiler",
        )?;
    }

    // 2. Update LLVM submodule to use blueshift fork
    println!("[2/5] Updating LLVM submodule...");
    let llvm_submodule_url = get_submodule_url(&rust_dir, "src/llvm-project")?;

    if llvm_submodule_url == LLVM_REPO {
        println!("  LLVM submodule already points to blueshift repo, skipping re-add");
        run_command(
            Command::new("git")
                .args(["submodule", "update", "--init", "--recursive", "src/llvm-project"])
                .current_dir(&rust_dir),
            "update llvm submodule",
        )?;
    } else {
        println!("  Switching LLVM submodule to blueshift repo...");
        // Remove existing submodule directories
        let modules_dir = rust_dir.join(".git/modules/src/llvm-project");
        if modules_dir.exists() {
            std::fs::remove_dir_all(&modules_dir)?;
        }
        let llvm_dir = rust_dir.join("src/llvm-project");
        if llvm_dir.exists() {
            std::fs::remove_dir_all(&llvm_dir)?;
        }

        // Re-add with blueshift repo
        run_command(
            Command::new("git")
                .args(["submodule", "add", "-f", LLVM_REPO, "src/llvm-project"])
                .current_dir(&rust_dir),
            "add llvm submodule",
        )?;

        run_command(
            Command::new("git")
                .args(["submodule", "update", "--init", "--recursive", "src/llvm-project"])
                .current_dir(&rust_dir),
            "update llvm submodule",
        )?;
    }

    // Checkout the correct branch in LLVM submodule
    let llvm_dir = rust_dir.join("src/llvm-project");
    run_command(
        Command::new("git")
            .args(["checkout", "-B", "BPF_i128_ret", "origin/BPF_i128_ret"])
            .current_dir(&llvm_dir),
        "checkout LLVM BPF_i128_ret branch",
    )?;

    // 3. Commit submodule update if needed
    println!("[3/5] Committing submodule update...");
    run_command(
        Command::new("git")
            .args(["add", "src/llvm-project"])
            .current_dir(&rust_dir),
        "stage llvm submodule",
    )?;

    let diff_status = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(&rust_dir)
        .status()?;

    if !diff_status.success() {
        run_command(
            Command::new("git")
                .args(["commit", "-m", "TMP: update submodule to BPF_i128_ret"])
                .current_dir(&rust_dir),
            "commit llvm submodule update",
        )?;
    } else {
        println!("  No changes to commit");
    }

    // 4. Configure and build Rust compiler
    println!("[4/5] Building Rust compiler (this may take a while)...");

    // Create bootstrap.toml if it doesn't exist
    let config_path = rust_dir.join("bootstrap.toml");
    if !config_path.exists() {
        println!("  Creating bootstrap.toml...");
        let config = r#"change-id = 148803
[llvm]

# Currently, we only support this when building LLVM for the build triple.
#
# Note that many of the LLVM options are not currently supported for
# downloading. Currently only the "assertions" option can be toggled.
download-ci-llvm = false

ninja = true
optimize = true
"#;
        std::fs::write(&config_path, config)
            .context("failed to write rust-compiler/bootstrap.toml")?;
    }

    run_command(
        Command::new("./x")
            .args(["build"])
            .current_dir(&rust_dir),
        "build rust compiler",
    )?;

    // 5. Link toolchain with rustup
    println!("[5/5] Linking toolchain with rustup...");
    let stage_dir = rust_dir.join("build/host/stage0");
    run_command(
        Command::new("rustup")
            .args(["toolchain", "link", TOOLCHAIN_NAME])
            .arg(&stage_dir),
        "link rustup toolchain",
    )?;

    println!("  Toolchain linked as '{}'", TOOLCHAIN_NAME);
    Ok(())
}

fn build_project(project_root: &Path) -> Result<()> {
    println!("Building project with custom toolchain...");
    run_command(
        Command::new("cargo")
            .args([&format!("+{}", TOOLCHAIN_NAME), "build-bpf"])
            .current_dir(project_root),
        "build project",
    )?;
    println!("Build complete!");
    Ok(())
}

fn get_submodule_url(repo_dir: &Path, submodule_path: &str) -> Result<String> {
    let output = Command::new("git")
        .args([
            "config",
            "--file",
            ".gitmodules",
            &format!("submodule.{}.url", submodule_path),
        ])
        .current_dir(repo_dir)
        .output()
        .context("failed to get submodule url")?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_command(cmd: &mut Command, description: &str) -> Result<()> {
    let status = cmd
        .status()
        .with_context(|| format!("failed to run: {}", description))?;

    if !status.success() {
        bail!("command failed: {}", description);
    }

    Ok(())
}
