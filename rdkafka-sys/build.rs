extern crate num_cpus;
extern crate pkg_config;
#[cfg(feature = "cmake_build")]
extern crate cmake;

use std::path::{Path, PathBuf};
use std::process::{Command, self};
use std::io::Write;
use std::env;

macro_rules! println_stderr(
    ($($arg:tt)*) => { {
        let r = writeln!(&mut ::std::io::stderr(), $($arg)*);
        r.expect("failed printing to stderr");
    } }
);

fn run_command_or_fail<P>(dir: &str, cmd: P, args: &[&str])
where
    P: AsRef<Path>,
{
    let cmd = cmd.as_ref();
    let cmd = if cmd.components().count() > 1 && cmd.is_relative() {
        // If `cmd` is a relative path (and not a bare command that should be
        // looked up in PATH), absolutize it relative to `dir`, as otherwise the
        // behavior of std::process::Command is undefined.
        // https://github.com/rust-lang/rust/issues/37868
        PathBuf::from(dir).join(cmd).canonicalize().expect("canonicalization failed")
    } else {
        PathBuf::from(cmd)
    };
    println_stderr!("Running command: \"{} {}\" in dir: {}", cmd.display(), args.join(" "), dir);
    let ret = Command::new(cmd).current_dir(dir).args(args).status();
    match ret.map(|status| (status.success(), status.code())) {
        Ok((true, _)) => { return },
        Ok((false, Some(c))) => { panic!("Command failed with error code {}", c) },
        Ok((false, None)) => { panic!("Command got killed") },
        Err(e) => { panic!("Command failed with error: {}", e) },
    }
}

fn main() {
    let librdkafka_version = env!("CARGO_PKG_VERSION")
        .split('-')
        .next()
        .expect("Crate version is not valid");

    if env::var("CARGO_FEATURE_DYNAMIC_LINKING").is_ok() {
        println_stderr!("librdkafka will be linked dynamically");
        let pkg_probe = pkg_config::Config::new()
            .cargo_metadata(true)
            .atleast_version(librdkafka_version)
            .probe("rdkafka");

        match pkg_probe {
            Ok(library) => {
                println_stderr!("librdkafka found on the system:");
                println_stderr!("  Name: {:?}", library.libs);
                println_stderr!("  Path: {:?}", library.link_paths);
                println_stderr!("  Version: {}", library.version);
            }
            Err(_) => {
                println_stderr!("librdkafka {} cannot be found on the system", librdkafka_version);
                println_stderr!("Dynamic linking failed. Exiting.");
                process::exit(1);
            }
        }
    } else {
        if !Path::new("librdkafka/LICENSE").exists() {
            println_stderr!("Setting up submodules");
            run_command_or_fail("../", "git", &["submodule", "update", "--init"]);
        }
        println_stderr!("Building and linking librdkafka statically");
        build_librdkafka();
    }

    let bindings = bindgen::Builder::default()
        .header("librdkafka/src/rdkafka.h")
        .generate_comments(false)
        .emit_builtins()
        // TODO: using rustified_enum is somewhat dangerous, especially when
        // also using shared libraries.
        // For details: https://github.com/rust-lang/rust-bindgen/issues/758
        .rustified_enum("rd_kafka_vtype_t")
        .rustified_enum("rd_kafka_type_t")
        .rustified_enum("rd_kafka_conf_res_t")
        .rustified_enum("rd_kafka_resp_err_t")
        .rustified_enum("rd_kafka_timestamp_type_t")
        .rustified_enum("rd_kafka_admin_op_t")
        .rustified_enum("rd_kafka_ResourceType_t")
        .rustified_enum("rd_kafka_ConfigSource_t")
        .generate()
        .expect("failed to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("failed to write bindings");
}

#[cfg(not(feature = "cmake_build"))]
fn build_librdkafka() {
    let mut configure_flags = Vec::new();

    if env::var("CARGO_FEATURE_GSSAPI").is_ok() {
        configure_flags.push("--enable-gssapi");
    } else {
        configure_flags.push("--disable-gssapi");
    }

    if env::var("CARGO_FEATURE_SSL").is_ok() {
        configure_flags.push("--enable-ssl");
    } else {
        configure_flags.push("--disable-ssl");
    }

    if env::var("CARGO_FEATURE_ZSTD").is_ok() {
        configure_flags.push("--enable-zstd");
    } else {
        configure_flags.push("--disable-zstd");
    }

    if env::var("CARGO_FEATURE_EXTERNAL_LZ4").is_ok() {
        configure_flags.push("--enable-lz4-ext");
    } else {
        configure_flags.push("--disable-lz4-ext");
    }

    println!("Configuring librdkafka");
    run_command_or_fail("librdkafka", "./configure", configure_flags.as_slice());

    println!("Compiling librdkafka");
    make_librdkafka();

    println!("cargo:rustc-link-search=native={}/librdkafka/src",
             env::current_dir().expect("Can't find current dir").display());
    println!("cargo:rustc-link-lib=static=rdkafka");
}

#[cfg(not(target_os= "freebsd"))]
fn make_librdkafka() {
    run_command_or_fail("librdkafka", "make", &["-j", &num_cpus::get().to_string(), "libs"]);
}

#[cfg(target_os= "freebsd")]
fn make_librdkafka() {
    run_command_or_fail("librdkafka", "gmake", &["-j", &num_cpus::get().to_string(), "libs"]);
}

#[cfg(feature = "cmake_build")]
fn build_librdkafka() {
    env::set_var("NUM_JOBS", num_cpus::get().to_string());
    let mut config = cmake::Config::new("librdkafka");
    config.define("RDKAFKA_BUILD_STATIC", "1")
          .build_target("rdkafka");
    if env::var("CARGO_FEATURE_SSL").is_ok() {
        config.define("WITH_SSL", "1");
    } else {
        config.define("WITH_SSL", "0");
    }
    if env::var("CARGO_FEATURE_SASL").is_ok() {
        config.define("WITH_SASL", "1");
    } else {
        config.define("WITH_SASL", "0");
    }
    if env::var("CARGO_FEATURE_ZSTD").is_ok() {
        config.define("WITH_ZSTD", "1");
        config.register_dep("zstd");
    } else {
        config.define("WITH_ZSTD", "0");
    }
    if env::var("CARGO_FEATURE_EXTERNAL_LZ4").is_ok() {
        config.define("ENABLE_LZ4_EXT", "1");
    } else {
        config.define("ENABLE_LZ4_EXT", "0");
    }
    if let Ok(system_name) = env::var("CMAKE_SYSTEM_NAME") {
        config.define("CMAKE_SYSTEM_NAME", system_name);
    }
    let dst = config.build();
    println!("cargo:rustc-link-search=native={}/build/src", dst.display());
    println!("cargo:rustc-link-lib=static=rdkafka");
}
