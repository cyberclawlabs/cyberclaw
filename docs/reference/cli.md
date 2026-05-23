# CLI Reference

当前 CLI 入口定义在：

- [apps/cyberclaw-cli/src/main.rs](../../apps/cyberclaw-cli/src/main.rs)

## 顶层命令

- `status`
- `inspect`
- `task`
- `connector`
- `package`
- `agent`
- `skill`
- `capability`

## 建议用法

```bash
cargo run -p cyberclaw-cli -- --help
cargo run -p cyberclaw-cli -- status
```

如果你要做扩展开发，继续看：

- [Builder Guide](../builders/README.md)
