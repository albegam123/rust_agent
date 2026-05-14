#[test]
fn print_env() {
    for (key, value) in std::env::vars() {
        println!("{}={}", key, value);
    }
    panic!("打印环境变量后停止"); // 确保输出被显示
}
