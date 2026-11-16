import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.Collections;
import java.util.Comparator;
import java.util.HashMap;
import java.util.HashSet;
import java.util.LinkedList;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.function.Function;
import java.util.function.Supplier;
import java.util.stream.Collectors;
import java.util.stream.Stream;

public class GenericAssignments {
    private final List<String> names = new ArrayList<>();
    private final Map<String, Integer> ages = new HashMap<>();
    private final Optional<String> nickname = Optional.empty();

    static class Box<T> {
        private final T value;

        Box(T value) {
            this.value = value;
        }

        T get() {
            return value;
        }

        Optional<T> maybe() {
            return Optional.ofNullable(value);
        }
    }

    static class Registry<K extends Comparable<K>, V> {
        private final Map<K, List<V>> entries = new HashMap<>();

        List<V> lookup(K key) {
            return entries.getOrDefault(key, Collections.emptyList());
        }

        Set<K> keys() {
            return entries.keySet();
        }
    }

    List<String> copyNames() {
        List<String> copy = new ArrayList<>(names);
        return copy;
    }

    Map<String, Integer> snapshot() {
        Map<String, Integer> result = new HashMap<String, Integer>(ages);
        return result;
    }

    Optional<String> firstName() {
        Optional<String> first = names.stream().findFirst();
        return first;
    }

    Box<String> boxed() {
        Box<String> box = new Box<String>("x");
        return box;
    }

    Set<String> unique(Collection<String> input) {
        Set<String> set = new HashSet<String>(input);
        return set;
    }

    List<Integer> lengths() {
        List<Integer> out = new LinkedList<Integer>();
        for (String name : names) {
            out.add(name.length());
        }
        return out;
    }

    Supplier<String> supplier() {
        Supplier<String> s = () -> "value";
        return s;
    }

    int run(Registry<String, Integer> registry, List<? extends Number> numbers) {
        List<String> local = names;
        Collection<String> view = local;
        Iterable<String> iterable = view;
        Map<String, Integer> map = ages;
        Set<String> keys = map.keySet();
        Collection<Integer> values = map.values();
        Optional<String> nick = nickname;
        Box<String> box = boxed();
        String unboxed = box.get();
        Optional<String> maybe = box.maybe();
        List<Integer> found = registry.lookup("k");
        Set<String> registered = registry.keys();
        List<? extends Number> nums = numbers;
        Number firstNumber = nums.isEmpty() ? 0 : nums.get(0);
        Stream<String> stream = local.stream();
        Stream<Integer> mapped = local.stream().map(s -> s.length());
        Function<String, Integer> length = String::length;
        Comparator<String> byLength = Comparator.comparing(length);
        List<String> sorted = new ArrayList<>(local);
        sorted.sort(byLength);
        Map<Integer, List<String>> grouped = local.stream().collect(Collectors.groupingBy(String::length));
        List<Integer> fixed = Arrays.asList(1, 2, 3);
        List<String> empty = Collections.emptyList();
        Map<String, List<Integer>> nested = new HashMap<String, List<Integer>>();
        nested.put("a", found);
        int total = 0;
        for (String key : keys) {
            total += key.length();
        }
        for (Integer v : values) {
            total += v;
        }
        for (Map.Entry<String, List<Integer>> entry : nested.entrySet()) {
            total += entry.getValue().size();
        }
        total += nick.map(String::length).orElse(0);
        total += unboxed.length() + maybe.map(String::length).orElse(0);
        total += registered.size() + firstNumber.intValue();
        total += (int) stream.count() + (int) mapped.count();
        total += grouped.size() + fixed.size() + empty.size() + sorted.size();
        for (String s : iterable) {
            total += s.length();
        }
        return total + copyNames().size() + snapshot().size() + firstName().map(String::length).orElse(0)
                + unique(local).size() + lengths().size() + supplier().get().length();
    }
}
