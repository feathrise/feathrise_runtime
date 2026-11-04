//! 只有会话计数的中性作者插件，不写工程文件、不冒充保存成功。
use feathrise_plugin_api::{Action, NativeTool, Request, SessionConfig, Snapshot};
pub struct CounterTool { count: u64 }
impl NativeTool for CounterTool {
    fn create(config: SessionConfig) -> Result<Self, String> {
        if config.plugin_id != "example.counter" { return Err("示例插件身份不匹配".into()); }
        Ok(Self { count: 0 })
    }
    fn request(&mut self, request: Request) -> Snapshot {
        if let Request::Action { action, .. } = request && action == "increment" {
            self.count = self.count.saturating_add(1);
        }
        Snapshot {
            plugin_id: "example.counter".into(), tool: "计数示例".into(), revision: self.count,
            status: format!("本次会话计数：{}", self.count),
            actions: vec![Action { id: "increment".into(), label: "增加一次".into(), enabled: true }],
            ..Default::default()
        }
    }
}
feathrise_plugin_api::export_plugin!(CounterTool, embedded);
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_sessions_keep_their_own_state() {
        let make = || CounterTool::create(SessionConfig {
            plugin_id: "example.counter".into(), project_id: "fixture".into(),
            project_root: String::new(), asset_root: String::new(),
        }).unwrap();
        let (mut first, mut second) = (make(), make());
        first.request(Request::Action { document: 0, action: "increment".into(), value: String::new() });
        assert_eq!(first.request(Request::Open).revision, 1);
        assert_eq!(second.request(Request::Open).revision, 0);
    }
}
