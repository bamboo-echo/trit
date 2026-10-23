//! 三值脉冲神经元网络（离散时间、整数膜电位、`{-1,0,+1}` 突触）。
//!
//! 每个 tick：膜电位先按 `decay` 泄漏，再叠加当前输入；越过 `±threshold` 就发一个
//! `+1`/`-1` 脉冲并把膜电位清零，随后进入 `refractory` 个 tick 的不应期。
//! 权重三值意味着连接只有「兴奋 / 无 / 抑制」三档，没有可调强度——这正是仿生侧
//! 关心的稀疏性来源。

/// 一个脉冲网络。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    /// `weights[i][j]`：`j` 到 `i` 的突触，取值 `-1/0/1`。
    pub weights: Vec<Vec<i8>>,
    /// 触发脉冲的绝对值阈值。
    pub threshold: i32,
    /// 泄漏：每 tick 膜电位先乘 `decay_num / decay_den`。
    pub decay: (i32, i32),
    /// 不应期 tick 数。
    pub refractory: u32,
}

impl Network {
    /// 随机稀疏网络（确定性种子）。
    pub fn random(size: usize, seed: u64) -> Self {
        let mut state = seed | 1;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let weights = (0..size)
            .map(|_| {
                (0..size)
                    .map(|_| match next() % 5 {
                        0 => 1,
                        1 => -1,
                        _ => 0,
                    })
                    .collect()
            })
            .collect();
        Self {
            weights,
            threshold: 3,
            decay: (3, 4),
            refractory: 1,
        }
    }

    /// 神经元数。
    pub fn size(&self) -> usize {
        self.weights.len()
    }
}

/// 一个 tick 之后的状态。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    /// 膜电位。
    pub membrane: Vec<i32>,
    /// 剩余不应期。
    pub refractory: Vec<u32>,
    /// 本 tick 的脉冲（`-1/0/1`）。
    pub spikes: Vec<i32>,
}

/// 一次模拟的统计。
#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    /// tick 数。
    pub ticks: usize,
    /// 脉冲总数。
    pub spikes: usize,
    /// 其中兴奋 / 抑制脉冲数。
    pub positive: usize,
    /// 抑制脉冲数。
    pub negative: usize,
    /// 任意时刻发过脉冲的神经元比例。
    pub active_fraction: f64,
    /// 每个 tick 的脉冲数。
    pub per_tick: Vec<usize>,
}

impl Stats {
    /// 平均每个神经元每 tick 的脉冲数（稀疏度指标）。
    pub fn spikes_per_neuron_tick(&self, neurons: usize) -> f64 {
        if neurons == 0 || self.ticks == 0 {
            return 0.0;
        }
        self.spikes as f64 / (neurons * self.ticks) as f64
    }
}

/// 跑 `ticks` 步；初始膜电位全 0，无外部输入。
pub fn simulate(network: &Network, ticks: usize) -> Stats {
    simulate_with_drive(network, ticks, &[])
}

/// 同 [`simulate`]，但每个 tick 额外给所有神经元加一个外部驱动。
pub fn simulate_with_drive(network: &Network, ticks: usize, external: &[i32]) -> Stats {
    let size = network.size();
    let mut state = State {
        membrane: vec![0; size],
        refractory: vec![0; size],
        spikes: vec![0; size],
    };
    let mut stats = Stats {
        ticks,
        spikes: 0,
        positive: 0,
        negative: 0,
        active_fraction: 0.0,
        per_tick: Vec::with_capacity(ticks),
    };
    let mut ever_spiked = vec![false; size];
    for tick in 0..ticks {
        let drive: Vec<i32> = (0..size)
            .map(|target| {
                network.weights[target]
                    .iter()
                    .zip(state.spikes.iter())
                    .map(|(weight, spike)| i32::from(*weight) * *spike)
                    .sum()
            })
            .collect();
        let mut fired = 0usize;
        for neuron in 0..size {
            if state.refractory[neuron] > 0 {
                state.refractory[neuron] -= 1;
                state.spikes[neuron] = 0;
                state.membrane[neuron] = state.membrane[neuron] * network.decay.0 / network.decay.1;
                continue;
            }
            let leaked = state.membrane[neuron] * network.decay.0 / network.decay.1;
            let kick = external.get(tick).copied().unwrap_or(0);
            let potential = leaked + drive[neuron] + kick;
            let spike = if potential >= network.threshold {
                1
            } else if potential <= -network.threshold {
                -1
            } else {
                0
            };
            if spike > 0 {
                stats.positive += 1;
            } else if spike < 0 {
                stats.negative += 1;
            }
            if spike != 0 {
                fired += 1;
                ever_spiked[neuron] = true;
                state.membrane[neuron] = 0;
                state.refractory[neuron] = network.refractory;
            } else {
                state.membrane[neuron] = potential;
            }
            state.spikes[neuron] = spike;
        }
        stats.spikes += fired;
        stats.per_tick.push(fired);
    }
    stats.active_fraction = if size == 0 {
        0.0
    } else {
        ever_spiked.iter().filter(|fired| **fired).count() as f64 / size as f64
    };
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single(weight: i8) -> Network {
        Network {
            weights: vec![vec![weight]],
            threshold: 1,
            decay: (1, 1),
            refractory: 0,
        }
    }

    #[test]
    fn quiet_network_stays_quiet() {
        let network = Network {
            weights: vec![vec![0, 0], vec![0, 0]],
            threshold: 3,
            decay: (1, 2),
            refractory: 1,
        };
        let stats = simulate(&network, 10);
        assert_eq!(stats.spikes, 0);
        assert_eq!(stats.active_fraction, 0.0);
    }

    #[test]
    fn external_kick_starts_self_excitation() {
        let stats = simulate_with_drive(&single(1), 3, &[1]);
        assert_eq!(stats.per_tick, vec![1, 1, 1]);
        assert_eq!(stats.positive, 3);
        assert_eq!(stats.spikes_per_neuron_tick(1), 1.0);
    }

    #[test]
    fn negative_drive_produces_inhibitory_spike() {
        let stats = simulate_with_drive(&single(0), 3, &[-1, 0, 0]);
        assert_eq!(stats.per_tick, vec![1, 0, 0]);
        assert_eq!(stats.negative, 1);
        assert_eq!(stats.positive, 0);
    }

    #[test]
    fn refractory_blocks_consecutive_spikes() {
        let mut network = single(0);
        network.refractory = 1;
        let stats = simulate_with_drive(&network, 4, &[1, 1, 1, 1]);
        assert_eq!(stats.per_tick, vec![1, 0, 1, 0]);
    }
}
