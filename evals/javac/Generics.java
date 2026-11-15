import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.function.Function;

public class Generics {
    static class Pair<A, B> {
        final A first;
        final B second;

        Pair(A first, B second) {
            this.first = first;
            this.second = second;
        }

        <C> Pair<A, C> withSecond(C next) {
            return new Pair<>(first, next);
        }

        Pair<B, A> swap() {
            return new Pair<>(second, first);
        }
    }

    static <T extends Comparable<T>> T max(List<? extends T> items) {
        T best = items.get(0);
        for (T item : items) {
            if (item.compareTo(best) > 0) {
                best = item;
            }
        }
        return best;
    }

    static <K, V> Map<V, K> invert(Map<K, V> source) {
        Map<V, K> result = new HashMap<>();
        for (Map.Entry<K, V> entry : source.entrySet()) {
            K key = entry.getKey();
            V value = entry.getValue();
            result.put(value, key);
        }
        return result;
    }

    static <T, R> List<R> mapAll(List<T> items, Function<? super T, ? extends R> f) {
        List<R> out = new ArrayList<>();
        for (T item : items) {
            R mapped = f.apply(item);
            out.add(mapped);
        }
        return out;
    }

    public int run(List<String> names) {
        Pair<String, Integer> pair = new Pair<>("a", 1);
        Pair<Integer, String> swapped = pair.swap();
        Pair<String, Double> other = pair.withSecond(2.0);
        String longest = max(names);
        List<Integer> lengths = mapAll(names, String::length);
        Map<Integer, String> inverted = invert(Map.of("x", 1));
        List<String> sorted = new ArrayList<>(names);
        Collections.sort(sorted, Comparator.reverseOrder());
        int size = lengths.size() + inverted.size() + sorted.size();
        double second = other.second;
        return size + swapped.first + longest.length() + (int) second;
    }

    public void wildcard(List<? super Integer> sink, List<? extends Number> source) {
        Number head = source.get(0);
        sink.add(head.intValue());
        Object any = sink.get(0);
        System.out.println(any);
    }
}
