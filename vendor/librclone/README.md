# librclone

Rust bindings for [`librclone`](https://github.com/rclone/rclone/tree/master/librclone).

This vendored copy is maintained for Explorer. It automatically compiles rclone
as a library and exposes a small safe RPC wrapper.

| crate version | `rclone` version | MSRV | Minimum `go` version |
| --- | --- | --- | --- |
| `librclone = "0.10"` | v1.74.3 | 1.82 | 1.25 |
| `librclone = "0.9"` | v1.69.0 | 1.82 | 1.21 |
| `librclone = "0.8"` | v1.66.0 | 1.70 | 1.21 |
| `librclone = "0.7"` | v1.65.0 | 1.65 | 1.19 |
| `librclone = "0.6"` | v1.64.2 | 1.65 | 1.19 |
| `librclone = "0.5"` | v1.63.1 | 1.60 | 1.18 |
| `librclone = "0.4"` | v1.62.2 | 1.54 | 1.18 |
| `librclone = "0.3"` | v1.61.0 | 1.54 | 1.17 |
| `librclone = "0.2"` | v1.60.1 | 1.54 | 1.17 |
| `librclone = "0.1"` | v1.56.2 | 1.54 | 1.17 |

## Build Notes

Linux and macOS build rclone as a static archive with:

```ignore
go build --buildmode=c-archive -ldflags "-X github.com/rclone/rclone/fs.Version=v1.74.3" -o <OUT_DIR>/librclone.a github.com/rclone/rclone/librclone
```

Windows builds rclone as a shared DLL and loads it at runtime:

```ignore
go build --buildmode=c-shared -tags cmount -ldflags "-s -X github.com/rclone/rclone/fs.Version=v1.74.3" -o <OUT_DIR>/librclone.dll github.com/rclone/rclone/librclone
```

Windows mount support requires WinFsp with the Developer feature installed. If
`CPATH` is unset, the build script looks for headers at
`C:\Program Files (x86)\WinFsp\inc\fuse`; otherwise the existing `CPATH` must
include the WinFsp FUSE headers.

To regenerate `go.mod` and `go.sum` when updating rclone:

```ignore
cd librclone-sys
go get github.com/rclone/rclone/librclone@v1.74.3
go mod tidy
```
