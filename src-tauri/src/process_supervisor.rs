use std::{
    collections::HashMap,
    process::Child,
    sync::Mutex,
};

pub struct ProcessSupervisor {
    children: Mutex<HashMap<String, Child>>,
}

impl Default for ProcessSupervisor {
    fn default() -> Self {
        Self {
            children: Mutex::new(HashMap::new()),
        }
    }
}

impl ProcessSupervisor {
    pub fn track(&self, label: String, child: Child) -> Result<(), String> {
        let mut children = self.children.lock().map_err(|_| "process lock poisoned")?;
        if let Some(mut previous) = children.remove(&label) {
            let _ = previous.kill();
            let _ = previous.wait();
        }
        children.insert(label, child);
        Ok(())
    }

    pub fn cleanup_finished(&self) {
        let Ok(mut children) = self.children.lock() else {
            return;
        };
        children.retain(|_, child| matches!(child.try_wait(), Ok(None)));
    }

    pub fn terminate_all(&self) {
        let Ok(mut children) = self.children.lock() else {
            return;
        };

        for (_, mut child) in children.drain() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for ProcessSupervisor {
    fn drop(&mut self) {
        if let Ok(children) = self.children.get_mut() {
            for (_, mut child) in children.drain() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}
