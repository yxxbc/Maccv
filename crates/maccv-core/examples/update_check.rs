fn main() {
    let version = std::env::args().nth(1).unwrap_or_else(|| "0.1.0".into());
    println!("已安装版本: {version}");
    match maccv_core::check_for_update(&version, None, maccv_core::UpdateChannel::Stable) {
        maccv_core::UpdateStatus::UpToDate { current } => println!("  → 已是最新 ({current})"),
        maccv_core::UpdateStatus::Available(m) => {
            println!("  → 发现新版本 {}", m.version);
            println!("     tag        {}", m.tag);
            println!(
                "     最低系统   {}",
                m.minimum_system_version.as_deref().unwrap_or("?")
            );
            match m.download_for_host() {
                Some(d) => println!("     本机应下载 {}", d.url.rsplit('/').next().unwrap()),
                None => println!("     本机没有可用下载！"),
            }
            println!("     说明 {} 字符", m.notes.len());
        }
        maccv_core::UpdateStatus::Revoked { current, manifest } => {
            println!("  → 版本 {current} 已被撤回，需更新到 {}", manifest.version);
        }
        maccv_core::UpdateStatus::CheckFailed { reason } => println!("  → 检查失败: {reason}"),
    }
}
