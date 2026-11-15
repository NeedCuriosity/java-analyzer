import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.function.Function;
import java.util.stream.Collectors;

class Box<T extends Comparable<T>> {
    private final T value;
    Box(T value) { this.value = value; }
    T get() { return value; }
    <R extends Comparable<R>> Box<R> map(Function<? super T, ? extends R> f) {
        return new Box<>(f.apply(value));
    }
    static <E extends Comparable<E>> E max(List<? extends E> xs) {
        E best = xs.get(0);
        for (E x : xs) {
            if (x.compareTo(best) > 0) best = x;
        }
        return best;
    }
}

class Main {
    Map<String, List<Integer>> index = new HashMap<>();

    int run(List<String> names) {
        Box<String> box = new Box<>("a");
        Box<Integer> len = box.map(String::length);
        int n = len.get() + Box.max(List.of(1, 2, 3));
        index.computeIfAbsent("k", k -> new ArrayList<>()).add(n);
        List<String> sorted = names.stream()
                .filter(s -> !s.isEmpty())
                .sorted(Comparator.comparing(String::length).thenComparing(Comparator.naturalOrder()))
                .collect(Collectors.toList());
        Map<Integer, Long> counts = names.stream()
                .collect(Collectors.groupingBy(String::length, Collectors.counting()));
        String joined = String.join(",", sorted);
        long total = counts.values().stream().mapToLong(Long::longValue).sum();
        Integer[] boxed = { 1, 2 };
        int[] raw = new int[boxed.length];
        for (int i = 0; i < raw.length; i++) raw[i] = boxed[i];
        return joined.length() + (int) total + raw[0];
    }
}
//? diag none
