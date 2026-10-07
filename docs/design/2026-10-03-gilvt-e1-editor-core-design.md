# gilvt E1：编辑核心（`gilvt-editor`）

日期：2026-10-03
状态：已实现
路线：gilvt 内置通用文本编辑器的第 1 个子项目，后续 E2 视图、E3 增强、E4 大文件见 [`2026-10-03-gilvt-e2-editor-view-notes.md`](2026-10-03-gilvt-e2-editor-view-notes.md)

## 1. 目标与范围

在 gilvt 里编辑文本文件，不再跳转到外部编辑器。用户选定：**通用文本编辑器**（不只是配置文件），接受它的体验上限（无插件、无 LSP、无代码折叠）。

E1 只交付**不依赖 gpui 的纯逻辑库**：文本缓冲区、选区、编辑与撤销、编码与换行符、加载与原子保存、外部修改检测。它不画任何东西，不处理键盘、鼠标、输入法、剪贴板，也不做语法高亮；这些属于 E2。验收只靠单元测试。

## 2. Crate

`crates/gilvt-editor`，加入 workspace。依赖 `ropey`（rope；关闭默认特性、只开 `simd`，否则 `\r`、`\u{85}`、`\u{2028}` 也会被当成换行，而内部文本只有 `\n` 一种换行）、`encoding_rs`（只转码，不探测）、`chardetng`（探测非 BOM、非 UTF-8 文件的编码）、`unicode-segmentation`（字素簇）、`unicode-width`（显示列）；dev 依赖 `tempfile`。不依赖 gpui、gilvt-agent 或其他 gilvt crate。

选 rope 而不是行数组：E3 的多光标和 E4 的大文件都需要 O(log n) 的随机编辑，现在用 rope 可以避免以后重写缓冲区。

## 3. 类型

- `Buffer`：`Rope`、`LineEnding`（`Lf` / `CrLf` / `Cr`，保存时原样写回）、`Encoding`（`Utf8`、`Utf8Bom`、`Utf16Le`、`Utf16Be`，其余旧编码（GBK、Shift_JIS、windows-1252 等）由 `chardetng` 探测、`encoding_rs` 转码）、`revision: u64`（每次文本变化加 1，含撤销、重做，供视图判断是否重绘）、`disk: DiskState`。`dirty()` = 撤销栈位置不等于保存点（保存点只存在于 `History`，`save` / `save_as` 成功后移到当前位置），所以撤销回到保存点后 `dirty()` 自动为假；`revision` 不参与 `dirty()`。
- 内部文本统一为 `\n` 换行；`LineEnding` 只在加载时探测（取出现次数最多的一种；混合时记录为最多的一种并设置 `mixed_line_endings`），在保存时转换回去。
- `Position { line, col }`：`col` 以字素簇计，光标不会停在组合字符中间。`Buffer` 提供这些公开方法（越界输入一律夹紧，不会 panic）：`position(char)` / `char_index(pos)`（`Position ↔ char 偏移`）、`char_to_byte` / `byte_to_char`（UTF-8 字节偏移）、`char_to_utf16` / `utf16_to_char`（UTF-16 码元偏移，供 LSP 与平台 API）、`display_col(pos)`（用 `tab_width`，`unicode-width`，中日韩字符占 2 列，Tab 对齐到 `tab_width`）、`char_at_display_col(line, col)`（鼠标点击定位）、`line_len_chars(line)`（不含换行）、`word_range_at(idx)`（双击选词：同类字符连续段，空白则取空白段，空行与文档末尾为空区间）。以 rope 为参数的自由函数不对外公开。
- `Selection { anchor, head }`，`head` 是光标；两者是**字符偏移**（char index，不是字素列，也不是 `Position`），`Position` 只在需要显示或按行列定位时互转。`Buffer` 内部只存一个选区，`selection()` 返回它，`selections()` 返回长度为 1 的切片，给 E3 的多光标留接口而不破坏 API。
- `Edit { start, removed: String, inserted: String }`：所有修改的最小单位，也是撤销的最小单位。

## 4. 编辑与撤销

- 基本操作：插入文本、删除（Backspace / Delete，按字素簇）、替换选区、插入换行、删除选区。
- 移动 / 选择：左右（字素簇）、上下（保持目标显示列）、按词、行首 / 行尾、文档首 / 尾、全选；每种移动有「扩展选区」变体。
- 删除：删除到词首 / 词尾、删除到行尾、删除整行。
- 每个操作返回 `Change { start_line, old_lines, new_lines }`，E2 用它做增量重绘：修改前文本的 `start_line..start_line+old_lines` 行，对应修改后的 `start_line..start_line+new_lines` 行。`undo()` / `redo()` 一步可能含多个 `Edit`，所以返回覆盖整篇文档的范围（`start_line = 0`，`old_lines` / `new_lines` 为操作前后的总行数）。
- **撤销栈**：存 `Edit` 的组（`UndoGroup`），每组带撤销前后的选区。连续的单字符插入、或连续的单字符删除，在间隔小于 1 秒（可注入时钟，便于测试）且位置相接时合并为一组；粘贴、删除选区、换行各自独立成组。新编辑清空重做栈。撤销栈上限 1000 组，超出丢弃最旧的。
- 撤销 / 重做后恢复当时的选区。

## 5. 加载与保存

- `Buffer::open(path) -> Result<Buffer, EditorError>`：
  - 超过 **64 MB** 返回 `TooLarge`（E4 再调整）。
  - 探测编码：BOM（UTF-8 / UTF-16）优先；否则先判二进制：含 NUL 字节，或控制字节（Tab、换行、回车、换页、ESC 除外）超过 1%，判为 `Binary`；再尝试严格 UTF-8；失败再用 `chardetng` 猜编码，用 `encoding_rs` 解码。在猜出的编码里无法干净解码的字节，判为 `UnsupportedEncoding`。**旧编码文件还必须能原样写回**：解码后立刻用同一编码重新编码并与原字节比较，若有无法映射的字符或字节不同（例如 Shift_JIS 的 NEC 扩展 0xED40、GBK 的 GB18030 四字节序列、Big5-HKSCS），同样返回 `UnsupportedEncoding`，宁可拒绝打开，也不冒着悄悄改写未编辑文件的风险。
  - 探测换行符；记录 `mtime`、大小和内容哈希到 `DiskState`。
  - 文件不是普通文件（目录、设备、FIFO）返回 `NotAFile`。
  - 没有写权限仍然可以打开，`Buffer::read_only()` 为真，所有修改操作返回 `ReadOnly`。
- `Buffer::save()` / `save_overwrite()` / `save_as(path)`：编码回原编码，换行符回原换行符；写到同目录的临时文件再 `rename`（沿用仓库的原子写入约定），临时文件名为 `.<文件名前 100 字节>.gilvt-tmp-<pid>-<计数器>`（长文件名也能保存，同一文件的两次保存不会冲突）；目标已存在时临时文件以 0600 创建并在改名前复制目标权限，目标不存在时按进程 umask 创建（通常 0644）；写完 fsync。成功后把保存点移到当前撤销位置并更新 `DiskState`。临时文件在任何失败路径上都被清理。保存时编码无法表示的字符（例如把含 emoji 的文本存回 GBK）返回 `Unrepresentable { line, col }`，不写文件；行列从 0 开始，列以字符计（不是字素），并且是编辑器内部（已统一为 `\n` 换行的）文本里的坐标，不论文件原来是 LF、CRLF 还是 CR。
  - **目录不可写的回退**：创建临时文件因 `PermissionDenied` 失败、但目标文件已存在且可写时（文件 0644、目录 0555），改为原地写目标（截断、写入、fsync）。这条路径**不是**崩溃原子的；目标不存在或不可写时错误仍是 `PermissionDenied`。
  - **不覆盖外部修改**：`save()` 先调用 `check_external()`，结果是 `Modified` 时返回 `ModifiedOnDisk`，不写任何内容；`save_overwrite()` 跳过这个检查（给「仍然覆盖」按钮用）。`Deleted` 不阻止 `save()`（会重新创建文件）。`save_as(path)` 不做检查。
- `Buffer::check_external() -> ExternalState`：`Unchanged` / `Modified` / `Deleted`。先比 mtime 和大小，不同再比内容哈希，所以只改 mtime（touch）不算修改。缓冲区本身不监听文件，由 E2 在窗口获得焦点或定时调用。
- `Buffer::reload()`：丢弃缓冲区内容，用磁盘内容重新加载，清空撤销栈。

## 6. 错误

`EditorError`：`TooLarge { size, limit }`、`Binary`、`UnsupportedEncoding`、`NotAFile`、`ReadOnly`、`NoPath`（没有文件名的缓冲区调用 `save` / `reload`，应改用 `save_as`）、`ModifiedOnDisk`（`save()` 发现文件在磁盘上已被其他程序修改，未写入；用 `save_overwrite()` 强制覆盖）、`PermissionDenied`、`Unrepresentable { line, col }`（列以字符计，坐标为内部 `\n` 文本里的，见 §5）、`Io(std::io::Error)`。每一种都有给用户看的中文 `Display`，E2 直接用在横幅上。

## 7. 测试

全部是 `gilvt-editor` 的单元测试，不需要 GUI：

- **差分测试**：用固定种子生成一串随机编辑（插入、删除、替换、撤销、重做），同步施加到 `Buffer` 和一个朴素的 `Vec<char>` 参考实现，每步比较文本；撤销到底后文本必须等于初始文本，重做到底后必须等于最终文本。
- 字素簇：组合字符、emoji、ZWJ 序列、中日韩字符，光标移动和删除都不会拆开它们。
- 显示列：Tab 对齐、全角字符占 2 列。
- 撤销合并：连续输入合并、间隔超过 1 秒不合并、粘贴独立成组、撤销恢复选区、`dirty()` 随撤销回到保存点变回假。
- 编码：UTF-8、UTF-8 BOM、UTF-16LE / BE、GBK 往返；BOM 保存时保留；含 NUL 判二进制；NUL 字节或超过 1% 控制字节判二进制；旧编码文件不能原样写回（如 Shift_JIS 的 0xED40）或字节在猜出的编码里无法干净解码判 `UnsupportedEncoding`；不可表示字符保存时报错且不写文件。
- 换行符：LF / CRLF / CR 往返不变；混合换行符加载后仍然保存为最多的一种并设置标记。
- 保存：原子性（保存失败不改原文件、不留临时文件）、保留权限（0600 保持 0600）、符号链接保存到目标文件而不是替换链接、250 字节长文件名、可写文件在只读目录里原地保存、`save()` 拒绝覆盖外部修改而 `save_overwrite()` 可以。
- 外部修改：`touch` 不算修改，内容变化算 `Modified`，删除算 `Deleted`，`reload` 后恢复干净。
- 拒绝：超过 64 MB（用稀疏文件或注入的上限）、目录、只读文件。

## 8. 验收标准

- `cargo test -p gilvt-editor` 全部通过，包含上面每一类测试。
- `cargo build` 整个 workspace 通过，`gilvt-app` 不受影响（E1 不改它）。
- 公共 API 带文档注释，说明每个错误何时返回。

## 9. 不在 E1 里

绘制、键盘与鼠标映射、输入法、剪贴板、语法高亮、查找替换、多光标、软换行、大文件优化、崩溃恢复缓存。它们分别属于 E2–E4。
