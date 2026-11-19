//! 交互 shell 的选择规则。
//!
//! 归属：`server_admin` 模型内部文件组。拥有：**"用什么命令拉起一个交互 shell"**这一条规则。
//! 提供：`pick_shell(login)`。允许调用：标准库。
//!
//! 为什么单独一个文件：容器控制台（`docker exec …`）与服务器终端（登录 shell）用的是
//! 同一条规则，只在"要不要登录 shell"上不同。两处各写一份 `command -v bash …`
//! 迟早会漂移——比如哪天都该加 `-i`，却只改了一边。

/// 远端挑 shell：bash 在就用 bash，否则退回 sh。`login = true` 时用登录 shell。
///
/// 用双引号而不是单引号——整段命令由 SSH 直接 exec，外层不套 `sh -c '...'`，
/// 但保持「脚本正文无单引号」的习惯，免得以后被复制到别处踩坑。
///
/// **不要在命令里加任何插值**（`$USER`、`cd $DIR`）：整段是双引号包裹的单个参数，
/// 由远端的默认登录 shell 解析，csh / fish 会把 `$` 提前展开。
pub fn pick_shell(login: bool) -> String {
    let flag = if login { " -l" } else { "" };
    format!("sh -c \"command -v bash >/dev/null 2>&1 && exec bash{flag} || exec sh\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 容器控制台一直在用这条串。改这里等于改它的行为，必须逐字对得上。
    #[test]
    fn non_login_shell_is_byte_identical_to_the_old_constant() {
        assert_eq!(
            pick_shell(false),
            "sh -c \"command -v bash >/dev/null 2>&1 && exec bash || exec sh\""
        );
    }

    #[test]
    fn login_shell_asks_bash_for_a_login_shell() {
        let command = pick_shell(true);
        assert!(command.contains("exec bash -l"), "{command}");
        // 兜底仍在：没有 bash 的机器（alpine）要退到 sh，不能直接失败
        assert!(command.contains("|| exec sh"), "{command}");
    }

    /// 这两条串会被远端登录 shell 解析：出现 `$` / 反引号 / `!` 就会被提前展开。
    #[test]
    fn shell_command_has_no_interpolation_metacharacters() {
        for login in [false, true] {
            let command = pick_shell(login);
            for forbidden in ['$', '`', '!', '\''] {
                assert!(
                    !command.contains(forbidden),
                    "命令里不能出现 {forbidden:?}：{command}"
                );
            }
        }
    }
}
