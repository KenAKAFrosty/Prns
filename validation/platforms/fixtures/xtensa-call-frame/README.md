# Xtensa call-frame reproducer

This small `no_std` fixture checks outgoing stack alignment when a function passes
fifteen `u32` arguments. It depends only on `core`.
Six arguments use registers and nine use the outgoing stack.

ESP Rust 1.95.0.0 / LLVM 21.1.3 was observed to reserve 36 bytes for those outgoing
arguments with forced frame pointers, violating the target's 16-byte call
alignment. Disabling frame pointers avoided that lowering path. This explains
the shared shipping Xtensa policy's explicit no-frame-pointer requirement; it is
not a general compiler correctness guarantee.

## Reproduce

Copy `Cargo.toml.example` and `src/lib.rs` into a temporary standalone crate. From
that crate, compare the generated assembly with the installed ESP toolchain:

```sh
RUSTFLAGS='-C force-frame-pointers=yes' cargo +esp rustc --release \
  --target xtensa-esp32s3-none-elf -Zbuild-std=core,alloc \
  --target-dir target/forced -- --emit=asm
RUSTFLAGS='-C force-frame-pointers=no' cargo +esp rustc --release \
  --target xtensa-esp32s3-none-elf -Zbuild-std=core,alloc \
  --target-dir target/default -- --emit=asm
```

The external callee is deliberately unresolved: this is a code-generation
fixture, not a linked or executed firmware test. Record the compiler version and
effective flags with new results; keep generated assembly and reports outside
the source tree.

From the repository root, run
`python3 validation/platforms/xtensa-frame-audit.py ELF REPORT.json` to inspect a
linked firmware image. The [shared auditor](../../xtensa-frame-audit.py) inspects
function-scoped `MOVSP` patterns in a real ELF
using `xtensa-esp32s3-elf-objdump` (or `--objdump PATH`). It distinguishes known
misaligned immediate adjustments from unresolved dynamic adjustments. Its report
does not prove whole-program stack alignment or a runtime stack bound.

The affected lowering is described by the pinned compiler's
[Xtensa frame lowering](https://github.com/espressif/llvm-project/blob/e4569c12a2b74dfd5f1657910f1dbcdf774e9413/llvm/lib/Target/Xtensa/XtensaFrameLowering.cpp)
and inherited
[call-frame reservation policy](https://github.com/espressif/llvm-project/blob/e4569c12a2b74dfd5f1657910f1dbcdf774e9413/llvm/include/llvm/CodeGen/TargetFrameLowering.h).
Recheck both the fixture and actual firmware when changing compilers or flags.
