mod editorconfig;
mod jetbrains;
mod vscode;
mod zed;

pub use editorconfig::*;
pub use jetbrains::*;
pub use vscode::*;
pub use zed::*;

#[cfg(test)]
mod tests;
