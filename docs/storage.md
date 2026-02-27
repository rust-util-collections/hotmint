# Storage

Hotmint 提供两种 `BlockStore` 实现和一个 `PersistentConsensusState`，分别用于区块持久化和共识状态的崩溃恢复。

| 组件 | 用途 | 后端 |
|:-----|:-----|:-----|
| `MemoryBlockStore` | 开发 / 测试 | HashMap + BTreeMap |
| `VsdbBlockStore` | 生产环境 | vsdb MapxOrd |
| `PersistentConsensusState` | 共识状态崩溃恢复 | vsdb MapxOrd |

## vsdb 简介

[vsdb](https://crates.io/crates/vsdb) 是一个高性能嵌入式 KV 数据库，API 设计类似 Rust 标准集合（HashMap / BTreeMap），底层使用 MMDB（纯 Rust 内存映射数据库引擎），无 C 库依赖。

### 核心类型

Hotmint 使用的 vsdb v10.x 核心类型：

| 类型 | 说明 | 类比 |
|:-----|:-----|:-----|
| `MapxOrd<K, V>` | 有序 KV 存储 | `BTreeMap<K, V>` |
| `Mapx<K, V>` | 无序 KV 存储 | `HashMap<K, V>` |
| `Orphan<T>` | 单值持久化容器 | `Box<T>` on disk |

`MapxOrd` 的常用方法：

```rust
// 创建
let mut map: MapxOrd<u64, String> = MapxOrd::new();

// 写入
map.insert(&1, &"hello".into());

// 读取
let val: Option<String> = map.get(&1);
let exists: bool = map.contains_key(&1);

// 范围查询
let first: Option<(u64, String)> = map.first();
let last: Option<(u64, String)> = map.last();
let le: Option<(u64, String)> = map.get_le(&5);  // 最后一个 ≤ 5 的
let ge: Option<(u64, String)> = map.get_ge(&5);  // 第一个 ≥ 5 的

// 迭代
for (k, v) in map.iter() { /* ... */ }
for (k, v) in map.range(10..20) { /* ... */ }

// 删除
map.remove(&1);
map.clear();
```

### 序列化要求

- Key 需实现 `KeyEnDeOrdered`（有序编码）
- Value 需实现 `ValueEnDe`
- 实现了 serde `Serialize + Deserialize` 的类型自动满足上述要求

### 关键函数

```rust
// 设置数据目录（必须在任何 vsdb 操作之前调用，仅能调用一次）
vsdb::vsdb_set_base_dir("/var/lib/hotmint/data").unwrap();

// 获取当前数据目录
let dir = vsdb::vsdb_get_base_dir();

// 强制刷盘
vsdb::vsdb_flush();
```

## BlockStore Trait

```rust
pub trait BlockStore: Send + Sync {
    fn put_block(&mut self, block: Block);
    fn get_block(&self, hash: &BlockHash) -> Option<Block>;
    fn get_block_by_height(&self, h: Height) -> Option<Block>;

    /// Get blocks in [from, to] inclusive. Default iterates one-by-one.
    fn get_blocks_in_range(&self, from: Height, to: Height) -> Vec<Block> { /* default */ }

    /// Return the highest stored block height. Default returns genesis.
    fn tip_height(&self) -> Height { Height::GENESIS }
}
```

trait 返回 owned `Block` 值（非引用），因为 vsdb 数据存储在磁盘上，无法返回内存中的借用引用。这一设计使内存和持久化实现使用同一接口。

## MemoryBlockStore

内存实现，适用于测试、开发和短生命周期进程。

```rust
use hotmint::consensus::store::MemoryBlockStore;

let store = MemoryBlockStore::new();
// 自动包含 height 0 的 genesis block
```

内部结构：
- `by_hash: HashMap<BlockHash, Block>` — O(1) 哈希查找
- `by_height: BTreeMap<u64, BlockHash>` — 有序高度查找

## VsdbBlockStore

持久化区块存储，基于 vsdb `MapxOrd`。区块在进程重启后仍然存在。

```rust
use hotmint::storage::block_store::VsdbBlockStore;

let store = VsdbBlockStore::new();
// 自动包含 genesis block

// 检查区块是否存在
if store.contains(&block_hash) {
    // ...
}

// 显式刷盘
store.flush();
```

### 内部数据模型

```rust
pub struct VsdbBlockStore {
    by_hash: MapxOrd<[u8; 32], Block>,     // BlockHash → Block
    by_height: MapxOrd<u64, [u8; 32]>,     // Height → BlockHash
}
```

两个索引协同工作：
- `put_block()` 同时写入两个 map
- `get_block()` 直接从 `by_hash` 查找
- `get_block_by_height()` 先从 `by_height` 查到 hash，再从 `by_hash` 取 block

### 在 ConsensusEngine 中使用

```rust
use std::sync::{Arc, RwLock};
use hotmint::consensus::engine::SharedBlockStore;

let store: SharedBlockStore =
    Arc::new(RwLock::new(Box::new(VsdbBlockStore::new())));

let engine = ConsensusEngine::new(
    state,
    store,  // SharedBlockStore = Arc<RwLock<Box<dyn BlockStore>>>
    Box::new(network_sink),
    Box::new(app),
    Box::new(signer),
    msg_rx,
    None,
);
```

## PersistentConsensusState

关键共识状态（view number、locked QC、highest QC、committed height、current epoch）必须在崩溃后恢复以维护安全性。

### 内部数据模型

```rust
// 使用单个 MapxOrd 存储多个状态字段
pub struct PersistentConsensusState {
    store: MapxOrd<u64, StateValue>,
}

// 状态值枚举（serde 序列化后存储）
enum StateValue {
    View(ViewNumber),
    Height(Height),
    Qc(QuorumCertificate),
    Epoch(Epoch),
}

// 固定 key 常量
const KEY_CURRENT_VIEW: u64 = 1;
const KEY_LOCKED_QC: u64 = 2;
const KEY_HIGHEST_QC: u64 = 3;
const KEY_LAST_COMMITTED_HEIGHT: u64 = 4;
const KEY_CURRENT_EPOCH: u64 = 5;
```

### API

```rust
use hotmint::storage::consensus_state::PersistentConsensusState;

let mut pstate = PersistentConsensusState::new();

// 保存状态（通常在 view 切换或 commit 后调用）
pstate.save_current_view(ViewNumber(42));
pstate.save_locked_qc(&qc);
pstate.save_highest_qc(&highest_qc);
pstate.save_last_committed_height(Height(10));
pstate.save_current_epoch(&epoch);
pstate.flush();

// 加载状态（启动时 / 崩溃恢复）
let view = pstate.load_current_view();           // Option<ViewNumber>
let locked = pstate.load_locked_qc();            // Option<QuorumCertificate>
let highest = pstate.load_highest_qc();          // Option<QuorumCertificate>
let committed = pstate.load_last_committed_height(); // Option<Height>
let epoch = pstate.load_current_epoch();         // Option<Epoch>
```

### 崩溃恢复示例

```rust
use hotmint::consensus::state::ConsensusState;
use hotmint::storage::block_store::VsdbBlockStore;
use hotmint::storage::consensus_state::PersistentConsensusState;

fn recover_or_init(vid: ValidatorId, vs: ValidatorSet) -> (ConsensusState, VsdbBlockStore) {
    let store = VsdbBlockStore::new();
    let pstate = PersistentConsensusState::new();

    let mut state = ConsensusState::new(vid, vs);

    // 从持久化状态恢复
    if let Some(view) = pstate.load_current_view() {
        state.current_view = view;
    }
    if let Some(qc) = pstate.load_locked_qc() {
        state.locked_qc = Some(qc);
    }
    if let Some(qc) = pstate.load_highest_qc() {
        state.highest_qc = Some(qc);
    }
    if let Some(h) = pstate.load_last_committed_height() {
        state.last_committed_height = h;
    }
    if let Some(epoch) = pstate.load_current_epoch() {
        state.current_epoch = epoch;
    }

    (state, store)
}
```

## 数据目录配置

vsdb 默认将数据存储在进程工作目录下。有两种方式指定自定义路径：

### 环境变量

```bash
export VSDB_BASE_DIR=/var/lib/hotmint/data
```

### 编程式配置

```rust
// 必须在任何 vsdb 操作之前调用，且仅能调用一次
vsdb::vsdb_set_base_dir("/var/lib/hotmint/data").unwrap();
```

`vsdb_set_base_dir()` 接受 `impl AsRef<Path>`，若数据库已初始化则返回错误。

## Flush 语义

vsdb 的写入操作默认异步落盘（由操作系统调度刷盘时机）。调用 `vsdb_flush()` 可以强制将所有待写入数据同步刷到磁盘。

建议在以下场景调用：
- 关键共识状态变更后（view 切换、QC 更新、commit）
- 应用层 `on_commit()` 完成后
- 节点优雅关闭前

`VsdbBlockStore` 和 `PersistentConsensusState` 都提供了 `.flush()` 方法，内部调用 `vsdb::vsdb_flush()`。

## vsdb 高级特性

vsdb v10.x 除了基础 KV 存储外，还提供以下高级特性，可用于 Hotmint 未来的功能扩展：

### VerMap — 版本化存储

`VerMap` 实现 Git 模型的版本化存储，支持分支、提交、三路合并和回滚。

```rust
use vsdb::versioned::map::VerMap;

let mut m: VerMap<u32, String> = VerMap::new();
let main = m.main_branch();

m.insert(main, &1, &"hello".into())?;
m.commit(main)?;

let feat = m.create_branch("feature", main)?;
m.insert(feat, &1, &"updated".into())?;
m.commit(feat)?;

// 分支隔离变更
assert_eq!(m.get(main, &1)?, Some("hello".into()));
assert_eq!(m.get(feat, &1)?, Some("updated".into()));

// 三路合并
m.merge(feat, main)?;
```

潜在用途：应用状态的乐观执行和回滚。

### MptCalc / SmtCalc — Merkle 证明

`MptCalc`（Merkle Patricia Trie）和 `SmtCalc`（Sparse Merkle Tree）提供无状态 Merkle 根计算和证明生成。

`VerMapWithProof` 结合版本化存储和 Merkle 根计算，每次提交产生 32 字节的状态根。

潜在用途：
- 轻客户端状态验证
- 跨链状态证明
- 应用层状态承诺

## 实现自定义 BlockStore

要使用其他存储后端（如 SQLite、sled 或远程数据库）：

```rust
use hotmint::prelude::*;
use hotmint::consensus::store::BlockStore;

struct SqliteBlockStore {
    conn: rusqlite::Connection,
}

impl SqliteBlockStore {
    fn new(path: &str) -> Self {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS blocks (
                hash BLOB PRIMARY KEY,
                height INTEGER NOT NULL,
                data BLOB NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_height ON blocks(height);"
        ).unwrap();
        Self { conn }
    }
}

impl BlockStore for SqliteBlockStore {
    fn put_block(&mut self, block: Block) {
        let data = serde_cbor_2::to_vec(&block).unwrap();
        self.conn.execute(
            "INSERT OR REPLACE INTO blocks (hash, height, data) VALUES (?1, ?2, ?3)",
            (&block.hash.0[..], block.height.as_u64() as i64, &data),
        ).unwrap();
    }

    fn get_block(&self, hash: &BlockHash) -> Option<Block> {
        self.conn
            .query_row(
                "SELECT data FROM blocks WHERE hash = ?1",
                [&hash.0[..]],
                |row| {
                    let data: Vec<u8> = row.get(0)?;
                    Ok(serde_cbor_2::from_slice(&data).unwrap())
                },
            )
            .ok()
    }

    fn get_block_by_height(&self, h: Height) -> Option<Block> {
        self.conn
            .query_row(
                "SELECT data FROM blocks WHERE height = ?1",
                [h.as_u64() as i64],
                |row| {
                    let data: Vec<u8> = row.get(0)?;
                    Ok(serde_cbor_2::from_slice(&data).unwrap())
                },
            )
            .ok()
    }
}
```
