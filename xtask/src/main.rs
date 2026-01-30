use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

const LLVM_REPO: &str = "https://github.com/blueshift-gg/llvm-project.git";
const LLVM_BRANCH: &str = "BPF_i128_ret";
const LINKER_REPO: &str = "https://github.com/blueshift-gg/sbpf-linker";
const LINKER_BRANCH: &str = "u128_mul_libcall";
const GIT_DEPTH: &str = "1";

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
    /// Set up the complete toolchain (LLVM + sbpf linker)
    Setup,
    /// Clone and build the SBPF linker only
    BuildLinker,
    /// Clone and build LLVM with modified BPF backend
    BuildLlvm,
    /// Build the example project with the custom toolchain
    Build,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let project_root = project_root()?;

    match cli.command {
        Commands::Setup => {
            setup_llvm()?;
            setup_linker(&project_root)?;
            println!();
            println!("==========================================");
            println!("Setup complete!");
            println!();
            println!("Build this project with:");
            println!("  cargo +nightly build-bpf");
            println!("==========================================");
        }
        Commands::BuildLinker => {
            setup_linker(&project_root)?;
        }
        Commands::BuildLlvm => {
            setup_llvm()?;
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
                .args(["clone", "--depth", GIT_DEPTH, "--branch", LINKER_BRANCH, LINKER_REPO])
                .arg(&linker_dir),
            "clone sbpf-linker",
        )?;
    }

    // 2. Build SBPF linker with LLVM_PREFIX pointing to our custom LLVM
    let llvm_install_dir = base_dir.join("llvm-install");
    println!("[2/3] Building SBPF linker (LLVM_PREFIX={})...", llvm_install_dir.display());

    let mut cmd = Command::new("cargo");
    cmd.args(["install", "--path", "."])
        .env("LLVM_PREFIX", &llvm_install_dir)
        .current_dir(&linker_dir);

    // On macOS, use Homebrew's llvm for libc++, zlib, and zstd
    // (macOS doesn't provide static libraries, and building them from source is complex)
    if cfg!(target_os = "macos") {
        // Check if dependencies are installed, install if missing
        let llvm_installed = std::process::Command::new("brew")
            .args(["--prefix", "llvm"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        let zlib_installed = std::process::Command::new("brew")
            .args(["--prefix", "zlib"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        let zstd_installed = std::process::Command::new("brew")
            .args(["--prefix", "zstd"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !llvm_installed || !zlib_installed || !zstd_installed {
            println!("  Installing Homebrew dependencies (llvm, zlib, zstd)...");
            run_command(
                Command::new("brew").args(["install", "llvm", "zlib", "zstd"]),
                "install brew dependencies",
            )?;
        }

        // Get brew prefixes
        let llvm_prefix = std::process::Command::new("brew")
            .args(["--prefix", "llvm"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let zlib_prefix = std::process::Command::new("brew")
            .args(["--prefix", "zlib"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        let zstd_prefix = std::process::Command::new("brew")
            .args(["--prefix", "zstd"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        
        let llvm_lib_cxx = format!("{}/lib/c++", llvm_prefix);
        cmd.env("CXXSTDLIB_PATH", &llvm_lib_cxx);
        cmd.env("ZLIB_PATH", format!("{}/lib", zlib_prefix));
        cmd.env("LIBZSTD_PATH", format!("{}/lib", zstd_prefix));
    }

    run_command(&mut cmd, "build sbpf-linker")?;

    // 3. Update .cargo/config.toml with linker path
    println!("[3/3] Updating .cargo/config.toml with linker path...");
    let cargo_config_dir = project_root.join(".cargo");
    std::fs::create_dir_all(&cargo_config_dir)?;

    let config_content = format!(
        r#"[target.bpfel-unknown-none]
rustflags = [
    "-C", "linker={}",
    "-C", "panic=abort",
    "-C", "link-arg=--dump-module=llvm_dump",
    "-C", "link-arg=--llvm-args=-bpf-stack-size=4096",
    "-C", "relocation-model=static",
]

[alias]
build-bpf = "build --release --target bpfel-unknown-none -Zbuild-std=core,alloc"
"#,
        linker_bin.display()
    );

    std::fs::write(cargo_config_dir.join("config.toml"), config_content)
        .context("failed to write .cargo/config.toml")?;

    println!("  SBPF linker ready at: {}", linker_bin.display());
    Ok(())
}

fn setup_llvm() -> Result<()> {
    let base_dir = cache_dir();
    let llvm_src_dir = base_dir.join("llvm-project");

    println!("  LLVM will be built in: {}", base_dir.display());

    // Ensure cache directory exists
    std::fs::create_dir_all(&base_dir)?;

    // 1. Clone LLVM repo if needed
    println!("[1/2] Cloning LLVM...");
    if llvm_src_dir.exists() {
        println!("  llvm-project directory already exists, skipping clone");
    } else {
        run_command(
            Command::new("git")
                .args(["clone", "--depth", GIT_DEPTH, "--branch", LLVM_BRANCH, LLVM_REPO])
                .arg(&llvm_src_dir),
            "clone llvm-project",
        )?;
    }

    // 2. Build LLVM from source (skip if already built)
    let llvm_build_dir = base_dir.join("llvm-build");
    let llvm_install_dir = base_dir.join("llvm-install");
    let llvm_config = llvm_install_dir.join("bin/llvm-config");

    if llvm_config.exists() {
        println!("[2/2] LLVM already built (found {}), skipping", llvm_config.display());
    } else {
        println!("[2/2] Building LLVM (this may take a while)...");
        std::fs::create_dir_all(&llvm_build_dir)?;
        std::fs::create_dir_all(&llvm_install_dir)?;
        build_llvm(&llvm_src_dir, &llvm_build_dir, &llvm_install_dir)?;
    }

    println!("  LLVM installed to: {}", llvm_install_dir.display());
    Ok(())
}

fn build_llvm(src_dir: &Path, build_dir: &Path, install_prefix: &Path) -> Result<()> {
    let mut install_arg = OsString::from("-DCMAKE_INSTALL_PREFIX=");
    install_arg.push(install_prefix.as_os_str());
    let mut cmake_configure = Command::new("cmake");
    let cmake_configure = cmake_configure
        .arg("-S")
        .arg(src_dir.join("llvm"))
        .arg("-B")
        .arg(build_dir)
        .args([
            "-G",
            "Ninja",
            "-DCMAKE_BUILD_TYPE=Release",
            "-DLLVM_BUILD_LLVM_DYLIB=ON",
            "-DLLVM_ENABLE_ASSERTIONS=ON",
            "-DLLVM_ENABLE_PROJECTS=",
            "-DLLVM_ENABLE_RUNTIMES=",
            "-DLLVM_INSTALL_UTILS=ON",
            "-DLLVM_LINK_LLVM_DYLIB=ON",
            "-DLLVM_TARGETS_TO_BUILD=BPF",
        ])
        .arg(install_arg);

    // On Linux, explicitly use clang to avoid C++ ABI mismatches with GCC
    if cfg!(target_os = "linux") {
        cmake_configure
            .arg("-DCMAKE_C_COMPILER=clang")
            .arg("-DCMAKE_CXX_COMPILER=clang++");
    }

    println!("Configuring LLVM with command {cmake_configure:?}");
    let status = cmake_configure.status().with_context(|| {
        format!("failed to configure LLVM build with command {cmake_configure:?}")
    })?;
    if !status.success() {
        anyhow::bail!("failed to configure LLVM build with command {cmake_configure:?}: {status}");
    }

    let mut cmake_build = Command::new("cmake");
    let cmake_build = cmake_build
        .arg("--build")
        .arg(build_dir)
        .args(["--target", "install"])
        // Create symlinks rather than copies to conserve disk space,
        // especially on GitHub-hosted runners.
        //
        // Since the LLVM build creates a bunch of symlinks (and this setting
        // does not turn those into symlinks-to-symlinks), use absolute
        // symlinks so we can distinguish the two cases.
        .env("CMAKE_INSTALL_MODE", "ABS_SYMLINK");
    println!("Building LLVM with command {cmake_build:?}");
    let status = cmake_build
        .status()
        .with_context(|| format!("failed to build LLVM with command {cmake_configure:?}"))?;
    if !status.success() {
        anyhow::bail!("failed to build LLVM with command {cmake_configure:?}: {status}");
    }

    // Move targets over the symlinks that point to them.
    //
    // This whole dance would be simpler if CMake supported
    // `CMAKE_INSTALL_MODE=MOVE`.
    for entry in WalkDir::new(install_prefix).follow_links(false) {
        let entry = entry.with_context(|| {
            format!(
                "failed to read filesystem entry while traversing install prefix {}",
                install_prefix.display()
            )
        })?;
        if !entry.file_type().is_symlink() {
            continue;
        }

        let link_path = entry.path();
        let target = fs::read_link(link_path)
            .with_context(|| format!("failed to read the link {}", link_path.display()))?;
        if target.is_absolute() {
            fs::rename(&target, link_path).with_context(|| {
                format!(
                    "failed to move the target file {} to the location of the symlink {}",
                    target.display(),
                    link_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn build_project(project_root: &Path) -> Result<()> {
    println!("Building project with cargo +nightly...");
    run_command(
        Command::new("cargo")
            .args(["+nightly", "build-bpf"])
            .current_dir(project_root),
        "build project",
    )?;
    println!("Build complete!");
    Ok(())
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
