# TUI 输入改进说明

## 问题诊断

### 原始问题
用户反馈：在某些终端环境下无法正常输入提示词

### 根本原因
原代码使用标准库的 `io::stdin().lock().read_line()` 进行输入，存在以下限制：

1. **无行编辑能力** - 无法编辑已输入的内容
2. **无命令历史** - 无法用 ↑↓ 键翻阅历史
3. **无快捷键支持** - Ctrl+C/A/E/K/U 等都不支持
4. **终端兼容性问题** - 不同终端行为不一致
5. **无光标控制** - 无法精确定位光标

## 解决方案

### 已实现的改进

#### 1. 完整的行编辑器 (`tui_input.rs`)
实现了现代化的行编辑器，支持：

**光标控制：**
- ✅ 左右方向键移动光标
- ✅ Home/End 键跳转到行首/行尾
- ✅ Backspace/Delete 删除字符

**命令历史：**
- ✅ ↑ 键上一条命令
- ✅ ↓ 键下一条命令
- ✅ 自动去重
- ✅ 最多保存 100 条历史

**快捷键：**
- ✅ Ctrl+C - 中断输入
- ✅ Ctrl+D - EOF
- ✅ Ctrl+A - 跳到行首
- ✅ Ctrl+E - 跳到行尾
- ✅ Ctrl+K - 删除光标后内容
- ✅ Ctrl+U - 删除光标前内容
- ✅ Ctrl+W - 删除前一个单词

**高级功能：**
- ✅ Tab 键支持（预留接口）
- ✅ 方向键历史导航
- ✅ 优雅的降级处理

#### 2. 终端模式管理
- 自动检测并启用 Raw Mode
- 支持 Alternate Screen
- 自动清理和恢复终端状态
- 异常时自动降级到基础模式

#### 3. 多模式支持
```rust
// TUI 模式（优先）
- 完整的功能支持
- 现代化的用户体验
- 自动失败处理

// 基础模式（降级）
- 标准 read_line()
- 兼容性保证
- 核心功能可用
```

## 技术架构

### 核心组件

#### `TuiInput` 结构体
```rust
pub struct TuiInput {
    stdout: Stdout,                    // 标准输出
    stdin: Stdin,                      // 标准输入
    history: VecDeque<String>,         // 历史记录
    history_index: Option<usize>,      // 当前历史位置
}
```

#### 关键方法
- `enable_raw_mode()` - 启用原始模式
- `disable_raw_mode()` - 禁用原始模式
- `read_line(prompt)` - 读取一行输入
- `clear_history()` - 清空历史
- `get_history()` - 获取历史列表

### 事件处理流程
```rust
loop {
    1. 读取事件 (event::read())
    2. 匹配键码 (KeyCode)
    3. 更新缓冲区
    4. 重绘行
    5. 刷新输出
}
```

## 使用示例

### 正常启动
```bash
$ ragent
ragent v0.1.0 — type /help for commands, /quit to exit

>>> Hello world   # 使用左右键编辑，Home/End跳转
```

### 命令历史
```bash
>>> previous_command
>>> ↑  # 显示上一条
previous_command
>>> ↓  # 下一条
```

### 新增命令
```bash
>>> /history     # 查看历史记录
Command history:
  1: first_command
  2: second_command
```

## 依赖说明

### 已添加的依赖
```toml
# 在 workspace dependencies
ratatui = "0.29.0"      # TUI 框架（预留）
crossterm = "0.28.1"    # 跨平台终端控制

# 在 ragent-cli dependencies
crossterm = { workspace = true }  # 实际使用
```

### 为什么选择 crossterm？
1. **跨平台** - 支持 Linux/macOS/Windows
2. **轻量级** - 比 ncurses 更现代
3. **async 支持** - 可与 tokio 集成
4. **活跃维护** - 社区活跃，更新及时

## 向后兼容性

### 自动降级
如果 TUI 模式初始化失败（权限、终端不支持等），自动降级到基础模式：

```rust
match tui.enable_raw_mode() {
    Ok(_) => run_interactive_tui(...),
    Err(_) => run_interactive_basic(...),  // 降级处理
}
```

### 功能降级
- 基础模式功能完全可用
- 只是缺少高级编辑功能
- 不影响核心 agent 功能

## 未来扩展

### 可选功能（已预留接口）
1. **Tab 补全** - 文件路径、命令等
2. **语法高亮** - Markdown、代码等
3. **多行编辑** - 类似于 REPL
4. **自动提示** - 基于历史的智能补全

### ratatui 集成（预留）
虽然当前未使用 ratatui，但已添加到依赖，可用于：
- 进度条显示
- 表格展示
- 复杂 UI 布局

## 性能考虑

### 输入响应
- 事件驱动架构，无阻塞
- 即时响应用户输入
- 无额外性能开销

### 内存使用
- 历史记录最多 100 条
- 使用 VecDeque 自动淘汰旧记录
- 极小的内存占用

## 测试建议

### 测试场景
1. ✅ 基础输入输出
2. ✅ 命令历史（上↑下↓）
3. ✅ 光标移动（←→）
4. ✅ 删除操作（Backspace/Delete）
5. ✅ 快捷键（Ctrl+A/E/K/U）
6. ✅ 中断处理（Ctrl+C）
7. ✅ 多行历史

### 终端兼容性
- macOS Terminal
- iTerm2
- Linux tty
- SSH 远程连接
- tmux/screen

## 相关文件

### 修改的文件
- `crates/cli/src/main.rs` - 集成 TUI 输入
- `crates/cli/Cargo.toml` - 添加依赖（预留）

### 新增的文件
- `crates/cli/src/tui_input.rs` - TUI 输入组件
- `TUI_IMPROVEMENTS.md` - 本文档

## 贡献者注意事项

### 代码风格
- 遵循 Rust 2018+ edition
- 使用 `crossterm::execute!` 宏
- 错误处理使用 `anyhow::Result`
- 文档完整

### 调试技巧
```rust
// 启用 debug 输出
RUST_LOG=debug cargo run
```

## 总结

本次改进解决了终端输入的核心问题，提供了：
- ✅ 完整的行编辑能力
- ✅ 命令历史支持
- ✅ 现代化的交互体验
- ✅ 优雅的降级处理
- ✅ 良好的扩展性

**用户体验显著提升**，特别是：
- 不再出现"输入不了"的问题
- 支持命令历史翻阅
- 快捷键提升效率
- 跨终端兼容性更好