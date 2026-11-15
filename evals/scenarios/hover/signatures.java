import java.util.List;
import java.util.Map;

class Repo<T> {
    /** 按名字查找，找不到返回 null。 */
    public T /*@find*/find(String name, int limit) { return null; }

    protected static <K, V> Map<K, List<V>> /*@group*/group(List<V> items) { return null; }

    private final Map<String, T> /*@cache*/cache = null;

    enum Mode { /*@fast*/FAST, SLOW }

    void run(Repo<String> strings) throws java.io.IOException {
        String s = strings./*@call_find*/find("a", 1);
        Map<Integer, List<String>> g = /*@call_group*/group(List.of(s));
        int n = s./*@lib_call*/indexOf("x");
        Mode m = Mode./*@use_fast*/FAST;
    }
}
//? hover find ~ "T find(String name, int limit)"
//? hover find ~ "按名字查找"
//? hover call_find ~ "find(String name, int limit)"
//? hover group ~ "<K, V> Map<K, List<V>> group(List<V> items)"
//? hover cache ~ "Map<String, T> cache"
//? hover use_fast ~ "FAST"
//? hover lib_call ~ "int indexOf(String"
