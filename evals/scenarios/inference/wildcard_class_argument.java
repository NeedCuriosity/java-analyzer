// 和 Spring 的 `BeanFactory.getBean` 一样：实参是 `Class<? extends Gen>` 时，
// `<T> T getBean(Class<T>)` 的结果是 `Gen`（`T` 推断为捕获变量，取它的上界）
interface Gen { String head(); }

class Context {
    Object getBean(String name) { return null; }
    <T> T getBean(String name, Class<T> type) { return null; }
    Object getBean(String name, Object... args) { return null; }
    <T> T getBean(Class<T> type) { return null; }
    <T> T getBean(Class<T> type, Object... args) { return null; }
}

class Use {
    Context context;

    String run(Class<? extends Gen> kind) {
        Gen gen = context.getBean(kind);
        var bean = context.getBean(kind);
        /*@bean*/bean.head();
        return gen.head();
    }
}
//? hover bean ~ "Gen bean"
//? diag none
