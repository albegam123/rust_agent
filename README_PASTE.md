# 粘贴支持说明

## macOS 终端限制

macOS 自带的 Terminal.app 对 Command 键的处理方式特殊，可能不会将其作为修饰符发送给应用程序。因此 Command+V 可能无法正常工作。

## 推荐替代方案

### 1. 使用 iTerm2 (推荐) ⭐

iTerm2 是 macOS 上最流行的终端替代品，对 Bracketed Paste Mode 支持更好：

**安装：**
```bash
brew install --cask iterm2
```

**优势：**
- ✅ Command+V 完全支持
- ✅ Bracketed Paste Mode 开箱即用
- ✅ 更好的 Unicode 支持

### 2. 快捷键替代

| 快捷键 | 说明 |
|--------|------|
| `Ctrl+Shift+V` | 通用粘贴快捷键 |
| `Shift+Insert` | 部分终端支持 |
| 右键 → Paste | 鼠标操作 |

### 3. 检查终端设置

确保你的终端启用了 **Bracketed Paste Mode**：

```bash
# 检查是否启用
echo -e '\033[200~test\033[201~'
```

如果显示 "test" 而不是看到特殊字符，说明支持正常。

## 为什么 Command+V 可能不工作

1. **macOS Terminal.app** 不将 Command 键作为 SUPER 修饰符发送
2. Command 键在 Terminal.app 中用于菜单快捷键
3. 这是一个设计限制，不是代码问题

## 解决方案

1. **使用 iTerm2** (最简单)
2. **使用 Ctrl+Shift+V** (通用)
3. **右键粘贴** (可靠)
4. **鼠标中键粘贴** (在支持的应用中)

## 技术背景

- `Bracketed Paste Mode` 使用 ESC[200~ 和 ESC[201~ 标记
- crossterm 会在启用后捕获这些序列
- 大多数现代终端都支持，但 macOS Terminal.app 支持有限
