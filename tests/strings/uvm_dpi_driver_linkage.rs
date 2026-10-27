//! `include/uvm_dpi_xezim.cc`, the driver that builds Accellera's UVM DPI
//! sources into a `--dpi-lib` library, gives everything it compiles C
//! linkage, as the reference `uvm_dpi.cc` does.
//!
//! The UVM sources call the SV export `m__uvm_report_dpi` through a plain
//! `extern` declaration. The driver used to include them after its
//! `extern "C"` block had closed, so built as C++ that reference was mangled
//! (`_Z17m__uvm_report_dpiiPKcS0_iS0_i`) and the library could not be loaded
//! at all. Its unguarded `extern "C"` also kept it from building as C.
//!
//! The UVM sources are not in this repository, so the tests compile the real
//! driver against small stand-ins with the same shapes: a header without a
//! linkage specification, `uvm_common.c` reaching the export through
//! `extern` under `svSetScope`, and a regex source using C++'s `false`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("xezim_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const UVM_DPI_H: &str = r#"#ifndef UVM_DPI__H
#define UVM_DPI__H
#include <stdlib.h>
#include <string.h>
#include "vpi_user.h"
#include "svdpi.h"
void m_uvm_report_dpi(int severity, char* id, char* message, int verbosity, char* file, int linenum);
int uvm_hdl_check_path(char *path);
int uvm_hdl_read(char *path, p_vpi_vecval value);
int uvm_hdl_deposit(char *path, p_vpi_vecval value);
int uvm_hdl_force(char *path, p_vpi_vecval value);
int uvm_hdl_release_and_read(char *path, p_vpi_vecval value);
int uvm_hdl_release(char *path);
extern char* uvm_dpi_get_tool_name_c ();
#endif
"#;

const UVM_COMMON_C: &str = r#"extern void m__uvm_report_dpi(int,const char*,const char*,int,const char*, int);
void m_uvm_report_dpi(int severity, char* id, char* message, int verbosity, char* file, int linenum) {
  svScope old_scope = svSetScope(svGetScopeFromName("top"));
  m__uvm_report_dpi(severity, id, message, verbosity, file, linenum);
  svSetScope(old_scope);
}
"#;

const UVM_REGEX_CC: &str = r#"unsigned char uvm_re_compexecfree(const char* re, const char* str, unsigned char deglob, int* exec_ret) {
  int same = strcmp(re, str) == 0;
  (void)deglob;
  *exec_ret = same ? 0 : 1;
  return same ? true : false;
}
"#;

const UVM_SVCMD_DPI_C: &str = r#"extern char* uvm_dpi_get_tool_name_c () {
  s_vpi_vlog_info info;
  vpi_get_vlog_info(&info);
  return info.product;
}
"#;

const SV: &str = r#"
module top;
  export "DPI-C" function m__uvm_report_dpi;
  function void m__uvm_report_dpi(int severity, string id, string message, int verbosity,
                                  string file, int linenum);
    $display("T|report %0d %s %s %0d %s %0d", severity, id, message, verbosity, file, linenum);
  endfunction
  import "DPI-C" function void m_uvm_report_dpi(int severity, string id, string message,
                                                int verbosity, string file, int linenum);
  import "DPI-C" function string uvm_dpi_get_tool_name_c();
  import "DPI-C" function byte unsigned uvm_re_compexecfree(string re, string str,
                                                            byte unsigned deglob,
                                                            output int exec_ret);
  import "DPI-C" function int uvm_hdl_check_path(string path);
  logic [7:0] sig = 8'h5a;
  initial begin
    int r;
    byte unsigned ok;
    #1;
    m_uvm_report_dpi(2, "ID", "from C", 100, "f.c", 7);
    $display("T|tool=%s", uvm_dpi_get_tool_name_c());
    ok = uvm_re_compexecfree("ab", "ab", 0, r);
    $display("T|re ok=%0d exec=%0d", ok, r);
    $display("T|path %0d %0d", uvm_hdl_check_path("top.sig"), uvm_hdl_check_path("top.nope"));
  end
endmodule
"#;

/// Build the driver with `compiler` against the stand-ins, load it and
/// run the C -> SV export round trip.
fn build_and_run(tag: &str, compiler: &str, lang_args: &[&str]) {
    let d = scratch(tag);
    for (name, text) in [
        ("uvm_dpi.h", UVM_DPI_H),
        ("uvm_common.c", UVM_COMMON_C),
        ("uvm_regex.cc", UVM_REGEX_CC),
        ("uvm_svcmd_dpi.c", UVM_SVCMD_DPI_C),
        ("top.sv", SV),
    ] {
        std::fs::write(d.join(name), text).unwrap();
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let so = d.join("uvm.so");
    let mut cc = Command::new(compiler);
    cc.args(lang_args).args(["-shared", "-fPIC"]);
    if cfg!(target_os = "macos") {
        // The exported subroutines resolve when xezim loads the library.
        cc.arg("-Wl,-undefined,dynamic_lookup");
    }
    let build = cc
        .arg("-I")
        .arg(root.join("include"))
        .arg("-I")
        .arg(&d)
        .arg(root.join("include").join("uvm_dpi_xezim.cc"))
        .arg("-o")
        .arg(&so)
        .output()
        .unwrap_or_else(|e| panic!("failed to launch {compiler}: {e}"));
    assert!(
        build.status.success(),
        "{compiler} failed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let out = Command::new(env!("CARGO_BIN_EXE_xezim"))
        .arg("--dpi-lib")
        .arg(&so)
        .args(["--no-cache", "-s", "top", "top.sv"])
        .current_dir(&d)
        .output()
        .expect("failed to run xezim");
    let _ = std::fs::remove_dir_all(&d);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{text}");
    let got: Vec<&str> = text.lines().filter(|l| l.starts_with("T|")).collect();
    assert_eq!(
        got,
        [
            "T|report 2 ID from C 100 f.c 7",
            "T|tool=xezim",
            "T|re ok=1 exec=0",
            "T|path 1 0"
        ],
        "{text}"
    );
}

#[test]
fn uvm_dpi_driver_built_as_cpp_binds_the_report_export() {
    build_and_run("uvm_dpi_cxx", "c++", &["-std=c++17"]);
}

#[test]
fn uvm_dpi_driver_builds_as_c() {
    build_and_run("uvm_dpi_c", "cc", &["-x", "c", "-std=gnu11"]);
}
