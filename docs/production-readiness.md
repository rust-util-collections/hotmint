# 生产就绪度分析

本文档深入分析 Hotmint 在 validator 生命周期管理、staking 支持基础设施、网络动态性等方面的实现完整度，并提出具体的改进计划。

分析原则：**staking / 奖励 / 惩罚等业务逻辑由应用层实现（通过 Application trait），hotmint 作为共识框架需提供充分的基础设施（trait hook、数据流、状态管理）让上层能够方便地实现这些功能。**

---

## 总览

| 领域 | 状态 | 说明 |
|:-----|:-----|:-----|
| 核心共识协议 | ✅ 完整 | HotStuff-2 两链提交，正确实现 |
| 加权投票 | ✅ 完整 | 按 power 加权，quorum = ceil(2*total_power/3) |
| Proposer 信息 | ✅ 完整 | Block.proposer 可用于应用层奖励分配 |
| 动态 ValidatorSet 更新 | ✅ 已实现 | `end_block` 返回 `EndBlockResponse` 含 `validator_updates`，引擎在 view 边界切换 |
| Epoch 切换 | ✅ 已实现 | `ConsensusState.current_epoch`，`PersistentConsensusState` 持久化 epoch，`advance_view_to` 执行切换 |
| 双签检测 / 证据机制 | ✅ 已实现 | `VoteCollector` 检测双签并返回 `EquivocationProof`，`Application::on_evidence()` 回调 |
| Application 返回 ValidatorSet | ✅ 已实现 | `end_block` 返回 `EndBlockResponse { validator_updates }` |
| Application 访问 ValidatorSet | ✅ 已实现 | `BlockContext` 提供 `&ValidatorSet` 和 `EpochNumber` |
| 状态同步 / 区块同步 | ❌ 缺失 | 新节点无法追赶 |
| 动态 Peer 发现 | ❌ 缺失 | 静态 peer 配置 |
| RPC 查询能力 | ⚠️ 基础 | status（含 epoch）+ submit_tx |

---

## 1. 动态 ValidatorSet 更新

### 现状：✅ 已实现

采用 Tendermint 式 EndBlock 返回值方案。`Application::end_block()` 返回 `EndBlockResponse`：

```rust
pub struct EndBlockResponse {
    pub validator_updates: Vec<ValidatorUpdate>,
}

pub struct ValidatorUpdate {
    pub id: ValidatorId,
    pub public_key: PublicKey,
    pub power: u64,  // power=0 表示移除
}
```

共识引擎在 `try_commit` 中调用 `end_block`，检查返回值。若 `validator_updates` 非空，则调用 `ValidatorSet::apply_updates()` 构建新的 validator set，创建新 `Epoch`，并在 view 边界（`advance_view_to`）执行切换。

应用层可在 `deliver_tx()` 中处理 staking 交易（质押/退出），在 `end_block()` 中汇总变更并返回 `ValidatorUpdate` 列表。

---

## 2. Epoch 切换机制

### 现状：✅ 已实现

Epoch 已完全集成到共识引擎：

- `ConsensusState` 包含 `current_epoch: Epoch`，在 `ConsensusState::new()` 中初始化为 `Epoch::genesis(validator_set)`
- Epoch 切换由应用层触发：`end_block` 返回非空 `validator_updates` 时，`try_commit` 构建新 `Epoch`（`pending_epoch`）
- 新 epoch 在 view 边界生效（`advance_view_to` 中应用 `pending_epoch`，设置实际 `start_view`）
- `PersistentConsensusState` 提供 `save_current_epoch()` / `load_current_epoch()` 用于崩溃恢复
- `BlockContext` 向 Application 传递 `epoch: EpochNumber` 和 `validator_set: &ValidatorSet`
- `StatusInfo` API 返回当前 epoch 编号

---

## 3. 双签检测与证据机制

### 现状：✅ 已实现

`VoteCollector::add_vote()` 现在返回 `VoteResult { qc, equivocation }`，同时进行 QC 形成和双签检测：

- 在添加投票时，遍历已有的 `(view, vote_type)` 条目，检查同一 validator 是否对不同 `block_hash` 投过票
- 检测到双签时构造 `EquivocationProof`，包含两个冲突投票的 block hash 和签名

```rust
pub struct EquivocationProof {
    pub validator: ValidatorId,
    pub view: ViewNumber,
    pub vote_type: VoteType,
    pub block_hash_a: BlockHash,
    pub signature_a: Signature,
    pub block_hash_b: BlockHash,
    pub signature_b: Signature,
}
```

Application trait 提供 `on_evidence` 回调：

```rust
fn on_evidence(&self, _proof: &EquivocationProof) -> Result<()> { Ok(()) }
```

应用层可在 `on_evidence()` 中实现 slashing 逻辑，然后在 `end_block()` 中通过 `ValidatorUpdate { power: 0 }` 移除被惩罚的 validator。

---

## 4. Validator 加权投票

### 现状：✅ 已正确实现

- `ValidatorInfo.power: u64` — 每个 validator 有独立的投票权重
- `ValidatorSet::quorum_threshold()` = `ceil(2 * total_power / 3)` — BFT 安全阈值
- `has_quorum()` 按 power 累加判断，非按数量计数
- 有测试 `test_quorum_weighted()` 验证加权场景

**无需改动**，但需配合动态 ValidatorSet 更新才能支持 staking 场景下的 power 变更。

---

## 5. Proposer 信息与奖励基础设施

### 现状：✅ 基本可用

`Block.proposer: ValidatorId` 在提案时设置，Application 在 `on_commit(block)` 中可读取。

```
// crates/hotmint-types/src/block.rs
pub struct Block {
    pub proposer: ValidatorId,  // 提案者 ID
    // ...
}
```

应用层可在 `on_commit()` 中基于 `block.proposer` 分配区块奖励。

### 改进状态：✅ 已通过 BlockContext 解决

`BlockContext` 包含 `proposer: ValidatorId`，所有 Application 生命周期方法（`begin_block`、`end_block`、`on_commit`）均可通过 `ctx.proposer` 访问提案者信息。应用层无需自行推算。

---

## 6. Application 访问 ValidatorSet

### 现状：✅ 已实现（方案 B：BlockContext）

采用 `BlockContext` 结构体聚合所有上下文信息：

```rust
pub struct BlockContext<'a> {
    pub height: Height,
    pub view: ViewNumber,
    pub proposer: ValidatorId,
    pub epoch: EpochNumber,
    pub validator_set: &'a ValidatorSet,
}
```

所有 Application 生命周期方法使用 `BlockContext`：

```rust
fn begin_block(&self, ctx: &BlockContext) -> Result<()>;
fn end_block(&self, ctx: &BlockContext) -> Result<EndBlockResponse>;
fn on_commit(&self, block: &Block, ctx: &BlockContext) -> Result<()>;
fn create_payload(&self, ctx: &BlockContext) -> Vec<u8>;
fn validate_block(&self, block: &Block, ctx: &BlockContext) -> bool;
```

这一设计具有良好的扩展性——未来添加新字段不需要修改 trait 签名。

---

## 7. 状态同步

### 现状：❌ 缺失

- 无快照 (snapshot) 机制
- 无状态同步协议
- 新节点必须从 genesis 重放所有区块
- `BlockStore` 只提供 `get_block` / `get_block_by_height`，无批量查询
- `ConsensusMessage` 无同步相关消息类型
- `NetworkSink` 无点对点数据请求能力（request-response 协议存在但仅用于共识消息）

### 建议方案

**Phase 1：区块同步**
- 添加 `BlockSync` 协议，新节点向已有节点请求缺失的区块
- `BlockStore` 添加 `get_blocks_range(from: Height, to: Height) -> Vec<Block>`
- 利用现有的 request-response 网络协议传输区块

**Phase 2：状态快照**
- Application trait 添加 `create_snapshot()` / `restore_snapshot()` 方法
- 实现定期快照 + 增量同步
- 新节点先下载最近快照，再同步后续区块

---

## 8. 网络动态性

### 现状

- litep2p P2P 传输（TCP），支持广播和单播
- `PeerMap`（ValidatorId ↔ PeerId）在 `NetworkService::create()` 时静态配置
- 运行时不可添加/移除 peer
- 无动态发现协议（无 DHT / gossip peer exchange）
- 无 NAT 穿透
- 无优雅关闭

### 建议方案

1. **PeerMap 动态化**：支持运行时 add/remove peer
2. **Peer 交换协议**：节点间交换 peer 列表
3. **健康检查**：定期 ping/pong 检测节点存活
4. **与 ValidatorSet 更新联动**：当 ValidatorSet 变更时，自动更新 PeerMap

---

## 9. RPC API

### 现状

仅两个端点：
- `status` — validator_id, current_view, last_committed_height, mempool_size
- `submit_tx` — 提交交易到 mempool

### 建议补充

| 端点 | 说明 | 优先级 |
|:-----|:-----|:-------|
| `get_block(height)` | 按高度查区块 | 高 |
| `get_block_by_hash(hash)` | 按哈希查区块 | 高 |
| `get_validators()` | 当前 validator 集合 | 高 |
| `get_validator(id)` | 单个 validator 信息 | 中 |
| `get_epoch()` | 当前 epoch 信息 | 中 |
| `get_peers()` | 已连接 peer 列表 | 中 |
| `get_consensus_state()` | view, height, role | 中 |

---

## 分阶段实施建议

### Phase 1：动态 ValidatorSet 基础设施 ✅ 已完成

1. ✅ `ConsensusState` 添加 `current_epoch: Epoch`
2. ✅ `Application::end_block()` 返回 `EndBlockResponse`（含 `validator_updates`）
3. ✅ `ConsensusEngine` 实现 epoch 切换逻辑：`try_commit` 检测 validator updates → 构建新 Epoch → `advance_view_to` 切换 validator set
4. ✅ `PersistentConsensusState` 持久化 epoch（`save_current_epoch` / `load_current_epoch`）

### Phase 2：双签检测与证据机制 ✅ 已完成

1. ✅ `VoteCollector` 添加双签检测（遍历 `(view, vote_type)` 索引检测同一 validator 对不同 block 的投票）
2. ✅ 新增 `EquivocationProof` 类型（含双方 block_hash + signature）
3. ✅ `VoteCollector::add_vote()` 返回 `VoteResult { qc, equivocation }`
4. ✅ Application trait 添加 `on_evidence()` 回调

### Phase 3：Application 上下文增强 ✅ 已完成

1. ✅ 引入 `BlockContext` 结构体（height, view, proposer, epoch, validator_set）
2. ✅ `begin_block`、`end_block`、`on_commit`、`create_payload`、`validate_block` 均使用 `BlockContext`
3. ✅ Application 通过 `ctx.validator_set` 获得 `&ValidatorSet` 只读访问

### Phase 4：状态同步 (TODO)

**目标**：新 validator 节点能追赶到当前状态。

1. `BlockStore` 添加范围查询 `get_blocks_range()`
2. 新增 `BlockSync` 网络协议
3. 实现区块同步流程（请求缺失区块 → 重放 → 加入共识）
4. Application trait 添加 `create_snapshot()` / `restore_snapshot()`

**依赖改动**：hotmint-consensus, hotmint-network, hotmint-storage

### Phase 5：网络动态化 + RPC 扩展 (TODO)

**目标**：支持运行时 peer 管理和丰富的查询接口。

1. `PeerMap` 支持运行时增删
2. 与 ValidatorSet 变更联动
3. 补充 RPC 端点（区块查询、validator 查询、epoch 查询）

**依赖改动**：hotmint-network, hotmint-api

---

## 当前已达到生产标准的部分

以下子系统实现完整、测试覆盖良好，已达到生产环境使用标准：

- **HotStuff-2 共识协议**：view protocol + pacemaker + 两链提交规则
- **加权投票与 quorum 计算**：正确的 BFT 安全阈值
- **Ed25519 签名与 Blake3 哈希**：标准密码学实现
- **vsdb 持久化存储**：区块存储 + 共识状态崩溃恢复
- **litep2p P2P 网络**：广播 + 单播消息路由
- **Mempool**：FIFO 去重 + 容量限制
- **Application trait 完整生命周期**：begin_block → deliver_tx → end_block（含 validator updates） → on_commit，所有方法接收 BlockContext
- **动态 ValidatorSet + Epoch 切换**：应用层通过 EndBlockResponse 触发 validator set 变更，引擎在 view 边界执行 epoch 切换
- **双签检测与证据机制**：VoteCollector 检测 equivocation 并通过 on_evidence 回调应用层
- **Prometheus 指标采集**：blocks_committed, votes_sent, view_timeouts 等
