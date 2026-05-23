# Pre-compiled binaries

Each subdirectory holds a pre-built `cyberclaw-server` + `cyberclaw-cli`
pair targeting one platform. Run `cargo build --release` from source to
produce binaries for any other target.

```
bin/
└── <target-triple>/
    ├── cyberclaw-server
    └── cyberclaw-cli
```

SHA-256 hashes are recorded in `RELEASE_MANIFEST.md`.
