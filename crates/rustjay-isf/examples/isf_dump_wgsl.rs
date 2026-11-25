//! Prints the WGSL the ISF pipeline hands to wgpu, for one shader.
//!
//! The backend-specific half of the pipeline lives past this point (naga
//! spv-out for Vulkan, hlsl-out for DX12, msl-out for Metal), so when a shader
//! renders on one backend and not another, this is the common input to diff
//! against — feed it to the `naga` CLI to see what each backend is given.
//!
//! Run: cargo run --release -p rustjay-isf --example isf_dump_wgsl -- <shader.fs>

fn main() -> Result<(), String> {
    let path = std::env::args().nth(1).ok_or("usage: isf_dump_wgsl <shader.fs>")?;
    // header::parse, not isf::parse — the corpus has headers the upstream
    // crate rejects (empty IMPORTED arrays), and the runtime uses this one.
    let src = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let isf = rustjay_isf::header::parse(&src).map_err(|e| format!("{path}: {e}"))?;
    print!("{}", rustjay_isf::generate_wgsl(&isf, &src)?.wgsl);
    Ok(())
}
