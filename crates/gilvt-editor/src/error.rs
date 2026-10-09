//! What can go wrong opening, editing or saving a file. Every variant has a message the UI can show as is.

use std::fmt;
use std::io;

#[derive(Debug)]
pub enum EditorError {
    /// The file is bigger than the editor's limit (`file::MAX_FILE_SIZE` unless a smaller one was asked for).
    TooLarge { size: u64, limit: u64 },
    /// NUL bytes or too many control bytes: not text.
    Binary,
    /// The bytes are not valid in the encoding the file announces or the detector guessed.
    UnsupportedEncoding,
    /// A directory, device or FIFO.
    NotAFile,
    /// The buffer was opened read-only; every modifying call returns this.
    ReadOnly,
    /// `save` / `reload` on a buffer that has no file yet (use `save_as`).
    NoPath,
    /// `save` found that another program changed the file since it was loaded or last saved; nothing was
    /// written. `save_overwrite` saves anyway.
    ModifiedOnDisk,
    /// The OS refused to read or write the file or its directory.
    PermissionDenied,
    /// Saving would lose a character the file's encoding cannot hold. 0-based line, and column in chars.
    Unrepresentable { line: usize, col: usize },
    Io(io::Error),
}

impl fmt::Display for EditorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if gilvt_i18n::english() {
            return match self {
                EditorError::TooLarge { size, limit } => {
                    write!(f, "The file is too large ({size} bytes); the editor's limit is {limit} bytes")
                }
                EditorError::Binary => f.write_str("This is a binary file and cannot be edited"),
                EditorError::UnsupportedEncoding => f.write_str("Could not recognise this file's encoding"),
                EditorError::NotAFile => f.write_str("This is not a regular file"),
                EditorError::ReadOnly => f.write_str("The file is read-only"),
                EditorError::NoPath => f.write_str("The file has no name yet; use \"Save As\" first"),
                EditorError::ModifiedOnDisk => {
                    f.write_str("Another program changed the file on disk; saving will overwrite those changes")
                }
                EditorError::PermissionDenied => f.write_str("No permission to write this file"),
                EditorError::Unrepresentable { line, col } => write!(
                    f,
                    "The character at line {}, column {} cannot be saved in the file's original encoding",
                    line + 1,
                    col + 1
                ),
                EditorError::Io(e) => write!(f, "Could not read or write the file: {e}"),
            };
        }
        match self {
            EditorError::TooLarge { size, limit } => write!(f, "文件太大（{size} 字节），超过编辑上限 {limit} 字节"),
            EditorError::Binary => f.write_str("这是二进制文件，不能编辑"),
            EditorError::UnsupportedEncoding => f.write_str("无法识别这个文件的编码"),
            EditorError::NotAFile => f.write_str("这不是普通文件"),
            EditorError::ReadOnly => f.write_str("文件是只读的"),
            EditorError::NoPath => f.write_str("还没有文件名，请先「另存为」"),
            EditorError::ModifiedOnDisk => f.write_str("文件在磁盘上已被其他程序修改，保存会覆盖它们"),
            EditorError::PermissionDenied => f.write_str("没有权限写入这个文件"),
            EditorError::Unrepresentable { line, col } => {
                write!(f, "第 {} 行第 {} 列的字符无法用文件原来的编码保存", line + 1, col + 1)
            }
            EditorError::Io(e) => write!(f, "读写文件失败：{e}"),
        }
    }
}

impl std::error::Error for EditorError {}

impl From<io::Error> for EditorError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::PermissionDenied {
            EditorError::PermissionDenied
        } else {
            EditorError::Io(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_denied_io_errors_become_their_own_variant() {
        let e: EditorError = io::Error::from(io::ErrorKind::PermissionDenied).into();
        assert!(matches!(e, EditorError::PermissionDenied));
        let e: EditorError = io::Error::from(io::ErrorKind::NotFound).into();
        assert!(matches!(e, EditorError::Io(_)));
    }

    #[test]
    fn messages_are_user_facing_and_one_based() {
        assert_eq!(EditorError::Binary.to_string(), "这是二进制文件，不能编辑");
        assert_eq!(
            EditorError::Unrepresentable { line: 0, col: 4 }.to_string(),
            "第 1 行第 5 列的字符无法用文件原来的编码保存"
        );
        assert!(EditorError::TooLarge { size: 9, limit: 3 }.to_string().contains("超过编辑上限 3"));
    }

    #[test]
    fn messages_in_english() {
        use gilvt_i18n::{has_chinese, with_language, Language};
        let all = [
            EditorError::TooLarge { size: 9, limit: 3 },
            EditorError::Binary,
            EditorError::UnsupportedEncoding,
            EditorError::NotAFile,
            EditorError::ReadOnly,
            EditorError::NoPath,
            EditorError::ModifiedOnDisk,
            EditorError::PermissionDenied,
            EditorError::Unrepresentable { line: 0, col: 4 },
            EditorError::Io(io::Error::other("disk full")),
        ];
        let english: Vec<String> = with_language(Language::English, || all.iter().map(ToString::to_string).collect());
        assert!(english.iter().all(|s| !has_chinese(s)), "{english:?}");
        assert_eq!(english[8], "The character at line 1, column 5 cannot be saved in the file's original encoding");
        assert_eq!(all[1].to_string(), "这是二进制文件，不能编辑");
    }
}
