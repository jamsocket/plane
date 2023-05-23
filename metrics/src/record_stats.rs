use serde::{Deserialize, Serialize};
use std::boxed::Box;
use sysinfo;
use sysinfo::{CpuExt, DiskExt, SystemExt};

#[derive(Serialize, Deserialize, Debug)]
pub struct Stats {
    pub num_cpus: usize,
    pub cpu_usage: Vec<f32>,
    pub mem_available: u64,
    pub num_disks: usize,
    pub disk_available: Vec<u64>,
    pub disk_total: Vec<u64>,
    pub mem_total: u64,
}

pub fn record_stats(system: &mut sysinfo::System) -> Stats {
    system.refresh_cpu();
    system.refresh_memory();
    system.refresh_disks();
    let mem_available = system.available_memory();
    let num_cpus: usize = system.cpus().len();
    let mut cpu_usage: Vec<f32> = Vec::new();
    for cpu in system.cpus() {
        cpu_usage.push(cpu.cpu_usage());
    }
    let num_disks: usize = system.disks().len();
    let mut disk_available: Vec<u64> = Vec::with_capacity(num_disks);
    let mut disk_total: Vec<u64> = Vec::with_capacity(num_disks);
    for disk in system.disks() {
        disk_available.push(disk.available_space());
        disk_total.push(disk.total_space())
    }

    let mem_total = system.total_memory();

    Stats {
        num_cpus,
        cpu_usage,
        mem_available,
        mem_total,
        num_disks,
        disk_available,
        disk_total,
    }
}

pub type StatsRecorder = Box<dyn FnMut() -> Stats>;
pub fn get_stats_recorder() -> StatsRecorder {
    let mut system = sysinfo::System::new_all();
    Box::new(move || record_stats(&mut system))
}
