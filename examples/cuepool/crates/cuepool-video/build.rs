fn main() {
    println!("cargo:rerun-if-changed=src/d3d12va_layout.cpp");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }

    let mut build = cc::Build::new();
    build.cpp(true).file("src/d3d12va_layout.cpp");
    if let Some(include) = std::env::var_os("FFMPEG_DIR") {
        build.include(std::path::PathBuf::from(include).join("include"));
    }
    build.compile("cuepool_d3d12va_layout");
}
