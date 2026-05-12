//! Build script for syphon-core
//! 
//! Links the required Apple frameworks on macOS

fn main() {
    #[cfg(target_os = "macos")]
    {
        // Check for local framework first (for development without system install)
        // Try multiple possible locations
        let possible_paths = [
            std::path::PathBuf::from("../syphon-lib"),  // Correct path from syphon-core
            std::path::PathBuf::from("../lib"),          // Old path
            std::path::PathBuf::from("../../syphon-lib"), // From deeper nesting
        ];
        
        for path in &possible_paths {
            let framework_path = path.join("Syphon.framework");
            if framework_path.exists() {
                let canonical = framework_path.canonicalize().unwrap();
                let parent = canonical.parent().unwrap();
                println!("cargo:rustc-link-search=framework={}", parent.display());
                // Add rpath so the executable can find the framework at runtime
                println!("cargo:rustc-link-arg=-Wl,-rpath,{}", parent.display());
                println!("cargo:warning=Found Syphon framework at: {}", canonical.display());
                break;
            }
        }
        
        // Add standard system framework paths
        println!("cargo:rustc-link-search=framework=/Library/Frameworks");
        println!("cargo:rustc-link-search=framework=/System/Library/Frameworks");
        
        println!("cargo:rustc-link-lib=framework=Syphon");
        println!("cargo:rustc-link-lib=framework=IOSurface");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=CoreGraphics");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=Metal");
        println!("cargo:rustc-link-lib=framework=MetalKit");
        println!("cargo:rustc-link-lib=framework=OpenGL");
        
        // Tell cargo to rerun if this file changes
        println!("cargo:rerun-if-changed=build.rs");
    }
}
