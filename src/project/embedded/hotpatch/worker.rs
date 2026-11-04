//! 唯一后台补丁槽：核包、持文件租约及真实装载，不创建第二个工程进程或重跑初始化。
use super::{package::{self, RootLease}, types::{Outcome, Request}};
use std::{sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}, mpsc}, time::Duration};

pub(super) fn run(root: Arc<RootLease>, input: mpsc::Receiver<Request>, output: mpsc::SyncSender<Outcome>,
    stop: Arc<AtomicBool>, generation: Arc<AtomicU64>) {
    while !stop.load(Ordering::Acquire) {
        let request = match input.recv_timeout(Duration::from_millis(30)) {
            Ok(request) => request,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let mut uncertain = false;
        let result = (|| {
            let verified = package::verify(root.clone(), &request, &stop, &generation)?;
            package::current(&request, &stop, &generation)?;
            // 安全边界来自完整基线、受控纯函数差异、真实会话与包/DLL摘要。
            // Subsecond接口将装载和跳表提交合一；旧库不得卸载，活跃World/Local不被替换。
            let (result, indeterminate) = apply(&verified.table, &request);
            uncertain = indeterminate;
            result
        })();
        let mut outcome = Outcome { request, result };
        // 真实采用结果不能被下一份遥测覆盖；等待发生在唯一后台槽，停止仍可撤销投递。
        loop {
            if stop.load(Ordering::Acquire) { return; }
            match output.try_send(outcome) {
                Ok(()) => {
                    if uncertain {
                        // 外部提交破坏唯一写者合同后停止此补丁槽；结果仍先交还主App，不能声称旧逻辑有效。
                        stop.store(true, Ordering::Release);
                        return;
                    }
                    break;
                }
                Err(mpsc::TrySendError::Disconnected(_)) => return,
                Err(mpsc::TrySendError::Full(value)) => {
                    outcome = value; std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CommitState { Unchanged, Adopted, Indeterminate }

fn classify(before: Option<usize>, after: Option<usize>, belongs_to_request: bool) -> CommitState {
    if before == after { CommitState::Unchanged }
    else if after.is_some() && belongs_to_request { CommitState::Adopted }
    else { CommitState::Indeterminate }
}

fn apply(table: &subsecond::JumpTable, request: &Request) -> (Result<(), String>, bool) {
    // Subsecond 0.7.9每次提交都新建并泄漏旧Box；只记地址，不把其临时借用跨apply保留。
    let before = unsafe { subsecond::get_jump_table().map(|current| std::ptr::from_ref(current) as usize) };
    let attempted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        subsecond::apply_patch(table.clone())
    }));
    // 同一后台槽在调用完成后观察，完整核对本包实际重定位后的跳表，而非仅相信返回值。
    let after = unsafe { subsecond::get_jump_table().map(|current|
        (std::ptr::from_ref(current) as usize, belongs_to(current, table))) };
    let state = classify(before, after.map(|value|value.0), after.is_some_and(|value|value.1));
    let problem = match attempted {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(format!("装载工具返回错误：{error}")),
        Err(_) => Some("装载或补丁通知回调发生panic".into()),
    };
    match state {
        CommitState::Adopted => {
            if let Some(problem) = problem {
                // commit_patch先发布跳表再调用handler；回调异常不等于代码未采用。
                // 主App仍必须按真实采用进入First通知、epoch递增及成功回执，警告走原工程日志。
                eprintln!("函数体热更新 #{}（源码代次{}）代码已采用，但补丁通知阶段异常：{}；将继续发布主App采用回执。",
                    request.patch_id, request.source_generation, problem);
            }
            (Ok(()), false)
        }
        CommitState::Unchanged => (Err(format!("代码补丁未提交新跳表：{}",
            problem.unwrap_or_else(|| "工具返回成功但未观察到本包采用".into()))), false),
        CommitState::Indeterminate => (Err("代码跳表已变化，但不属于当前受检补丁；当前逻辑状态无法确认，补丁通道已停用，请重新连接预览。".into()), true),
    }
}

fn belongs_to(current: &subsecond::JumpTable, expected: &subsecond::JumpTable) -> bool {
    if current.lib != expected.lib || current.aslr_reference != expected.aslr_reference
        || current.new_base_address != expected.new_base_address || current.ifunc_count != expected.ifunc_count
        || current.map.len() != expected.map.len() { return false; }
    // 核包已要求aslr_reference等于实际运行引用，所以旧地址位移为0；由真实main推导新DLL位移。
    let Some(main) = current.map.get(&expected.aslr_reference) else { return false; };
    let offset = main.wrapping_sub(expected.new_base_address);
    expected.map.iter().all(|(source, target)| current.map.get(source) == Some(&target.wrapping_add(offset)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_identity_separates_no_commit_adoption_and_unknown_publication() {
        assert_eq!(classify(None,None,false),CommitState::Unchanged);
        assert_eq!(classify(Some(1),Some(1),false),CommitState::Unchanged);
        assert_eq!(classify(None,Some(2),true),CommitState::Adopted);
        assert_eq!(classify(Some(1),Some(2),true),CommitState::Adopted);
        assert_eq!(classify(Some(1),Some(2),false),CommitState::Indeterminate);
        assert_eq!(classify(Some(1),None,false),CommitState::Indeterminate);
    }

    #[test]
    fn actual_table_must_match_every_requested_address_after_one_dll_relocation() {
        let expected = subsecond::JumpTable { lib: std::path::PathBuf::from("owned-patch.dll"),
            map: [(10,100),(20,200)].into_iter().collect(), aslr_reference:10, new_base_address:100, ifunc_count:0 };
        let mut actual = expected.clone();
        for address in actual.map.values_mut() { *address += 4096; }
        assert!(belongs_to(&actual,&expected));
        actual.map.insert(20,4297);
        assert!(!belongs_to(&actual,&expected),"同main但其他函数不属于本包时不能报告成功");
        actual.map.insert(20,4296); actual.lib = "other-patch.dll".into();
        assert!(!belongs_to(&actual,&expected));
        actual.lib = expected.lib.clone(); actual.map.remove(&10);
        assert!(!belongs_to(&actual,&expected));
    }
}
