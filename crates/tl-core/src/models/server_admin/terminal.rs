//! 服务器终端：登录 shell 的启动命令。
//!
//! 归属：`server_admin` 模型内部文件组。拥有：**"用什么命令进这台机器"**这一条规则。
//! 提供：`login_shell_command()`。允许调用：兄弟模块 `shell`、标准库。
//!
//! 纪律：
//! - **不接收任何参数**：前端传进来的字符串一律不参与拼接，否则这条 WS
//!   就成了通用的"任意命令执行"接口；
//! - **身份不加工**：配置里是哪个用户就以哪个用户登录，界面不提供 root/su 档位。

use super::shell::pick_shell;

/// 登录 shell：bash 优先（`-l` 会读 profile，PATH 完整，最接近 `ssh` 上来），没有就 sh。
///
/// 交给 `engine::open_pty` 执行时 PTY 已分配、stdin 是 tty、没有 `-c` 与位置参数，
/// bash 会判定为交互式登录 shell；sshd 在 exec 前已 chdir 到用户家目录。
pub fn login_shell_command() -> String {
    pick_shell(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_uses_the_login_variant() {
        assert_eq!(login_shell_command(), pick_shell(true));
        assert!(login_shell_command().contains("exec bash -l"));
    }
}
