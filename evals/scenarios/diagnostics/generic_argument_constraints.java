import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;

class Main {
    static <T> void same(List<T> a, List<T> b) {
    }

    static <T> void put(T value, List<T> into) {
    }

    static <K, V> void merge(Map<K, V> target, Map<K, V> source) {
    }

    static <T> void pair(T a, T b) {
    }

    static <T> T[] fill(T[] array, T value) {
        return array;
    }

    void uses(List<String> strings, List<Integer> ints, List<Number> numbers, List<Object> objects,
              Map<String, Integer> counts, Map<String, Long> totals, String[] words) {
        /*@same*/same(strings, ints);
        /*@put*/put(1, strings);
        Collections./*@copy*/copy(ints, strings);
        /*@merge*/merge(counts, totals);

        same(strings, new ArrayList<String>());
        put(1, numbers);
        put("s", objects);
        Collections.copy(numbers, ints);
        Collections.copy(objects, strings);
        merge(counts, counts);
        pair("a", 1);
        pair(strings, ints);
        fill(words, "x");
        Collections.addAll(numbers, 1, 2L);
    }
}
//? diag argument-mismatch same
//? diag argument-mismatch put
//? diag argument-mismatch copy
//? diag argument-mismatch merge
//? diag exact
