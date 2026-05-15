// Pre-release feature gates. Flip back to `true` to expose a feature again.
//
// Projects/Tasks 的实现还有未修问题，发版前整体隐藏：
// 1. SessionList 不渲染 Projects rail + 新建/管理项目入口
// 2. 主区域不路由到 ProjectsView/TasksView（持久化的 viewMode 会被弹回 gallery）
// 3. SessionList 的 5s 轮询不再调用 projects/tasks store
// 4. ProjectsView/TasksView 自身 early-return，避免被其他路径意外挂载
export const PROJECTS_FEATURE_ENABLED = false;
