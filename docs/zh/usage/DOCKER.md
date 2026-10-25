# 清理本机 Docker

开发工具插件可通过 Docker Engine / Docker Desktop 的本机接口清理对象，支持 Linux、Windows、macOS 桌面。Android/iOS 不支持。先检查原生工具和所需子命令，只有接口可用时才枚举对象；缺 Docker 不扫描，缺 Buildx 只影响构建缓存。已安装但 daemon 不可达、权限不足、接口版本不兼容或选中远程端点时显示具体原因，不请求提权运行用户 Docker 命令。

建议先检查构建缓存与悬空镜像。清理构建缓存会增加后续构建与下载开销；悬空镜像必须未被任何容器引用。有标签的未使用镜像、已停止容器是另外两项手动可选能力：前者可能是离线保留镜像，后者包含可能尚未另存的数据和日志。

| Handler | 范围 | 默认选择 |
| --- | --- | --- |
| `builtin.developer-tools/docker-build-cache` | 本机 Engine 默认 Buildx 构建器中未在使用的缓存记录；不含自定义或远程 builder | 可选，可由显式 `recommend.enabled=true` 推荐 |
| `builtin.developer-tools/docker-dangling-images` | 未被任何容器引用的悬空镜像 | 可选，可由显式 `recommend.enabled=true` 推荐 |
| `builtin.developer-tools/docker-unused-images` | 未被任何容器引用的有标签镜像 | 始终手动可选 |
| `builtin.developer-tools/docker-stopped-containers` | exited、created、dead 状态的容器及其可写层 | 始终手动可选 |

```toml
[modules.docker]
name = "本机 Docker"
[modules.docker.tasks.build-cache]
name = "Docker 构建缓存"
handler = "builtin.developer-tools/docker-build-cache"
platforms = ["linux", "windows", "macos"]
[modules.docker.tasks.build-cache.recommend]
enabled = false
```

Docker 的 `options` 为空，不接受路径、shell、任意命令、builder 或提权配置；`recommend` 仅有 `enabled`，默认 `false`，不支持年龄或大小过滤。保存 Recipe 定义未来扫描范围；每次执行仍只作用于本次扫描并确认的完整对象 ID，不包含确认后新增对象。

每次扫描固定本机端点与 Engine ID，清理前重新检查两者及对象资格。仅接受本机 `unix://` socket 或 Windows `npipe:////./pipe/`；拒绝 TCP、SSH 及远程 named pipe，即使 TCP 地址写作 localhost。不会切换 context、启动 daemon/builder 或操作远程机器。需要切换本机 context 时由用户在 Docker 中完成，然后重新扫描。

清理使用精确 ID 的官方接口。镜像和容器不使用强制删除；镜像保留未选中的父镜像，运行容器不会被停止。Buildx 使用锚定 ID 过滤；不调用无范围 `system prune`。数据卷、bind mount、Docker 存储目录和 Docker Desktop 虚拟磁盘不属于清理范围。Docker 自身因依赖、并发状态或多标签拒绝删除时，保留失败/不确定结果，重新扫描后再决定。

镜像大小和容器可写层大小取自 Docker 的整数字节字段。Buildx 输出四舍五入的 kB/MB/GB 时大小保持未知；准确的整数字节保留已知值。共享层不可简单相加为可释放空间，Docker Desktop 虚拟磁盘也未必立即缩小。因此预估可释放字节与完成后的回收字节为未知，而不是 0；清理成功状态来自对象实际消失的复核。

API 调用者可用 `probe_system_capabilities` 获取可用能力及已安装接口的诊断，`issue` 不为空的结果不可签发清理来源。原有 `discover_system_capabilities` 继续只返回可用任务。App 把诊断作为不可执行的 inventory 提示；检查已知失败项仅返回诊断，不枚举对象或创建方案任务。
