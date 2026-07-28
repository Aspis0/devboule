M0 executed. Below is the required output and structured acceptance report.

## Commit

```
92a9ed64eca64a3c090cd03676353100d5cce2f2
```

## Diff stat

```
 src-tauri/Cargo.toml | 2 +-
 1 file changed, 1 insertion(+), 1 deletion(-)
```

Single-line change: `windows = "0.58"` feature list — 9 existing features preserved, 4 appended (`Win32_System_JobObjects`, `Win32_Security`, `Win32_System_Memory`, `Win32_NetworkManagement_WindowsFilteringPlatform`).

## `cargo check --target x86_64-pc-windows-msvc` (last lines, verbatim)

```
error: failed to run custom build command for `lance-encoding v8.0.0`

Caused by:
  process didn't exit successfully: `C:\Users\gualt\Desktop\devboule\src-tauri\target\debug\build\lance-encoding-f6ebb7d5e8fe9d89\build-script-build` (exit code: 1)
  --- stderr
  Error: Custom { kind: NotFound, error: "Could not find `protoc`. If `protoc` is installed, try setting the `PROTOC` environment variable to the path of the `protoc` binary. Try installing `protobuf-compiler` or `protobuf` using your package manager. It is also available at https://github.com/protocolbuffers/protobuf/releases  For more information: https://docs.rs/prost-build/#sourcing-protoc" }

error: failed to run custom build command for `lance-file v8.0.0`
  (same `protoc` NotFound error)
```

→ **Not caused by M0.** `protoc` (host build tool for `lance`) is not installed. This blocks *all* cargo checks in this repo, regardless of target. The `windows = "0.58"` feature resolution succeeded — cargo compiled past it into `lance`.

## `cargo tree -i windows`

```
error: specification `windows` is ambiguous
help: re-run this command with one of the following specifications
  windows@0.57.0
  windows@0.58.0
  windows@0.61.3
```

Trace of the 3rd version (not from M0):
```
windows v0.57.0
└── sysinfo v0.33.1
    ├── devboule v0.1.0
    └── oracle-core v0.1.0
        └── devboule v0.1.0
```
`0.57.0` is a **pre-existing transitive dep** via `sysinfo`. My change only added features to the `0.58.0` block; it did not introduce `0.57.0`. The oracle's pre-flight "exactly 2 versions" assumption was wrong about the baseline.