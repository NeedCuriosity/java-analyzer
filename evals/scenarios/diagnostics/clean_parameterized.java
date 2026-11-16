import java.io.Serializable;
import java.util.AbstractList;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.Collections;
import java.util.Comparator;
import java.util.EnumMap;
import java.util.EnumSet;
import java.util.HashMap;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.TreeMap;
import java.util.function.BiFunction;
import java.util.function.Function;
import java.util.function.Supplier;
import java.util.stream.Collector;
import java.util.stream.Collectors;
import java.util.stream.Stream;

enum Color { RED, GREEN }

class RawList extends ArrayList {
}

class IntList extends ArrayList<Integer> {
}

class Node<T extends Comparable<T>> implements Comparable<Node<T>> {
    T value;
    public int compareTo(Node<T> other) {
        return value.compareTo(other.value);
    }
}

class Holder<T extends Number> implements Supplier<T> {
    T value;
    public T get() {
        return value;
    }
}

class Pair<A, B> {
    A first;
    B second;
}

class Outer<T> {
    List<T> items = new ArrayList<>();
    class Inner {
        List<T> copy = items;
    }
}

class Base<T> {
    List<T> items = new ArrayList<>();
    List<T> items() {
        return items;
    }
}

class RawChild extends Base {
    void use() {
        List<String> a = items;
        List<String> b = items();
        List<String> c = this.items;
        System.out.println(a.size() + b.size() + c.size());
    }
}

class StringChild extends Base<String> {
    void use() {
        List<String> a = items;
        List<String> b = items();
        Collection<? extends CharSequence> c = items;
        System.out.println(a.size() + b.size() + c.size());
    }
}

abstract class Shape implements Comparable<Shape> {
}

class Square extends Shape {
    public int compareTo(Shape o) {
        return 0;
    }
}

class Gen<T extends Comparable<T>> {
    T value;
    List<T> items = new ArrayList<>();

    Comparable<T> self() {
        return value;
    }

    Comparable<? super T> lower() {
        return value;
    }

    Object any() {
        return value;
    }

    <U extends T> T up(U u) {
        return u;
    }

    List<? extends T> covariant() {
        return items;
    }

    Collection<T> collection() {
        return items;
    }

    Optional<T> maybe() {
        return Optional.ofNullable(value);
    }

    List<T> fresh() {
        return new ArrayList<>(items);
    }

    <E extends Enum<E>> EnumSet<E> none(Class<E> type) {
        return EnumSet.noneOf(type);
    }

    T[] same(T[] xs) {
        return xs;
    }

    Object[] widened(T[] xs) {
        Comparable<?>[] cs = xs;
        return cs;
    }

    Map<T, List<T>> index() {
        Map<T, List<T>> m = new HashMap<>();
        m.put(value, items);
        m.computeIfAbsent(value, k -> new ArrayList<>()).add(value);
        return m;
    }

    void each() {
        for (T t : items) {
            Comparable<T> c = t;
            System.out.println(c);
        }
        for (Comparable<T> c : items) {
            System.out.println(c);
        }
    }

    class Inner {
        List<T> copy = items;
        T first() {
            return items.get(0);
        }
    }

    static <V> Class<? extends V> recurse(Class<V> type) {
        Class<?> component = type.getComponentType();
        if (component != null) {
            recurse(component);
        }
        return type;
    }

    @SuppressWarnings("unchecked")
    T[] grow(T[] xs) {
        return (T[]) Arrays.copyOf(xs, xs.length + 1, xs.getClass());
    }

    Gen<T> copy() {
        Gen<T> g = new Gen<>();
        return g;
    }

    static <V> List<V> single(V v) {
        List<V> out = Collections.singletonList(v);
        return out;
    }
}

class Main {
    static <T extends Comparable<? super T>> void sort(List<T> list) {
        Collections.sort(list);
    }

    static <E extends Enum<E>> Set<E> all(Class<E> type) {
        return EnumSet.allOf(type);
    }

    static <T> T first(Class<? extends T> type, List<? extends T> xs) {
        return xs.get(0);
    }

    @SafeVarargs
    static <T> List<T> listOf(T... xs) {
        return Arrays.asList(xs);
    }

    Optional<? extends Number> number() {
        return Optional.of(1);
    }

    List<? super Integer> sink() {
        return new ArrayList<Number>();
    }

    Map<String, List<Integer>> index() {
        return new HashMap<>();
    }

    Comparator<String> comparator() {
        return Comparator.comparing(String::length).thenComparing(Comparator.naturalOrder());
    }

    void wildcards(List<? extends Number> numbers, List<? super Integer> sinks, List<?> any,
            Map<String, ? extends List<String>> lists, Class<?> type, Class<? extends Number> numType) {
        List<? extends Number> a = numbers;
        Collection<? extends Number> b = numbers;
        Iterable<? extends Object> c = numbers;
        List<?> d = numbers;
        List<? super Integer> e = sinks;
        Collection<? super Integer> f = sinks;
        List<?> g = any;
        Collection<?> h = any;
        Object i = any;
        Map<String, ? extends Collection<String>> j = lists;
        Map<? extends CharSequence, ? extends List<String>> k = lists;
        Class<?> l = numType;
        Class<? extends Number> m = Integer.class;
        Class<? extends Comparable<?>> n = String.class;
        Class<?> o = type;
        List<? extends Comparable<? extends Number>> p = new ArrayList<Integer>();
        Comparable<? super Integer> q = 1;
        System.out.println("" + a + b + c + d + e + f + g + h + i + j + k + l + m + n + o + p + q);
    }

    void captures(List<? extends Node<?>> nodes, Node<?> node, Holder<?> holder, Holder<? extends Integer> ih) {
        List<? extends Comparable<?>> a = nodes;
        Comparable<?> b = node;
        Supplier<? extends Number> c = holder;
        Supplier<? extends Integer> d = ih;
        Supplier<?> e = holder;
        Comparable<? extends Node<?>> f = node;
        System.out.println("" + a + b + c + d + e + f);
    }

    @SuppressWarnings({"unchecked", "rawtypes"})
    void raw(List rawList, Map rawMap, RawList rl, Class rawClass) {
        List<String> a = rawList;
        Map<String, Integer> b = rawMap;
        List<String> c = rl;
        List<Integer> d = new ArrayList();
        Collection<String> e = new RawList();
        Class<? extends Number> f = rawClass;
        List<List<String>> g = new ArrayList<List<String>>(rawList);
        Iterator<String> h = rawList.iterator();
        Set<Map.Entry<String, String>> i = rawMap.entrySet();
        List<String> j = (List) a;
        List<String> k = Collections.EMPTY_LIST;
        System.out.println("" + a + b + c + d + e + f + g + h + i + j + k);
    }

    void subclasses(IntList il, Square sq, Map<Color, String> byColor) {
        List<Integer> a = il;
        Collection<? extends Number> b = il;
        AbstractList<Integer> c = il;
        Comparable<Shape> d = sq;
        Comparable<? super Square> e = sq;
        List<Square> squares = new ArrayList<>();
        List<? extends Shape> f = squares;
        sort(squares);
        EnumMap<Color, String> g = new EnumMap<Color, String>(Color.class);
        Map<Color, String> h = g;
        Set<Color> i = all(Color.class);
        Serializable j = "x";
        Comparable<String> k = "x";
        CharSequence l = "x";
        Map<Color, String> m = new EnumMap<>(byColor);
        System.out.println("" + a + b + c + d + e + f + g + h + i + j + k + l + m);
    }

    void inference(List<String> names, Map<String, Integer> counts) {
        List<String> a = Collections.emptyList();
        List<String> b = new ArrayList<>(names);
        Map<String, List<String>> c = names.stream().collect(Collectors.groupingBy(s -> s.substring(0, 1)));
        Map<Integer, Long> d = names.stream().collect(Collectors.groupingBy(String::length, Collectors.counting()));
        Map<String, Integer> e = names.stream().collect(Collectors.toMap(s -> s, String::length));
        Map<String, Integer> f = names.stream().collect(Collectors.toMap(Function.identity(), String::length, Integer::sum, TreeMap::new));
        List<Integer> g = names.stream().map(String::length).collect(Collectors.toList());
        Stream<Integer> h = names.stream().map(s -> s.length() + 1);
        Optional<Integer> i = names.stream().map(String::length).max(Comparator.naturalOrder());
        List<Object> j = Arrays.asList("a", 1);
        List<Number> k = Arrays.asList(1, 2.0);
        List<Number> l = listOf(1, 2L);
        Number m = first(Integer.class, Arrays.asList(1, 2));
        Map<String, Integer> n = new LinkedHashMap<>(counts);
        List<Map.Entry<String, Integer>> o = new ArrayList<>(counts.entrySet());
        o.sort(Map.Entry.comparingByValue(Comparator.reverseOrder()));
        Supplier<List<String>> p = ArrayList::new;
        BiFunction<String, Integer, Pair<String, Integer>> q = (s, x) -> new Pair<>();
        Function<List<? extends Number>, Number> r = xs -> xs.get(0);
        List<? extends Number> s = Collections.singletonList(1);
        Set<String> t = counts.keySet();
        Collection<Integer> u = counts.values();
        Set<Map.Entry<String, Integer>> v = counts.entrySet();
        Optional<String> w = Optional.of("x").map(String::trim);
        Optional<CharSequence> x = Optional.<CharSequence>of("x");
        List<List<String>> y = List.of(names, List.of("z"));
        Map<String, Integer> z = Map.of("a", 1, "b", 2);
        System.out.println("" + a + b + c + d + e + f + g + h + i + j + k + l + m + n + o + p + q + r + s + t + u + v + w + x + y + z);
    }

    void lambdas(List<Integer> ints, Optional<Integer> maybe, Function<Integer, List<Integer>> wrap,
            List<? extends Number> numbers) {
        Stream<Object> a = ints.stream().map(i -> i);
        Stream<Number> b = ints.stream().map(i -> i * 2);
        Stream<String> c = ints.stream().map(i -> "#" + i);
        Optional<Long> d = maybe.map(x -> (long) x);
        Stream<Integer> e = ints.stream().map(i -> {
            if (i > 0) {
                return i;
            }
            return -i;
        });
        Stream<List<Integer>> f = ints.stream().map(wrap);
        Stream<Collection<Integer>> g = ints.stream().map(i -> wrap.apply(i));
        Optional<List<? extends Number>> h = maybe.map(wrap);
        Stream<Byte> k = ints.stream().map(i -> (byte) 1);
        Stream<Character> l = ints.stream().map(i -> 'c');
        Stream<int[]> m = ints.stream().map(i -> new int[i]);
        Stream<Object[]> n = ints.stream().map(i -> new String[i]);
        Comparator<Integer> o = Comparator.comparing(i -> -i);
        Map<Boolean, List<Integer>> p = ints.stream().collect(Collectors.partitioningBy(i -> i > 0));
        Stream<? extends Number> q = ints.stream().map(i -> i);
        Optional<Object> r = maybe.map(x -> x > 0 ? "a" : x);
        Stream<Number> t = numbers.stream().map(x -> x);
        Stream<Double> u = numbers.stream().map(x -> x.doubleValue());
        Optional<Comparable<Integer>> v = maybe.map(x -> x);
        Stream<Serializable> w = ints.stream().map(i -> i);
        Optional<Iterable<Integer>> x = maybe.map(wrap::apply);
        System.out.println("" + a + b + c + d + e + f + g + h + k + l + m + n + o + p + q + r + t + u + v + w + x);
    }

    static <T, K, V> Collector<T, ?, Map<K, V>> toMap(Function<? super T, ? extends K> keys,
            Function<? super T, ? extends V> values) {
        return Collector.of(
                HashMap<K, V>::new,
                (map, item) -> map.put(keys.apply(item), values.apply(item)),
                (a, b) -> {
                    a.putAll(b);
                    return a;
                });
    }

    static Collector<CharSequence, ?, String> concat() {
        return Collector.of(StringBuilder::new, StringBuilder::append, StringBuilder::append,
                StringBuilder::toString);
    }

    static <T> Collector<T, ?, List<T>> listOf() {
        return Collector.of(ArrayList::new, List::add, (a, b) -> {
            a.addAll(b);
            return a;
        });
    }

    void loops(Map<String, List<String>> index, List<? super Integer> sinks, List<? extends Number> nums) {
        for (Map.Entry<String, ? extends List<String>> e : index.entrySet()) {
            System.out.println(e);
        }
        for (Map.Entry<String, List<String>> e : index.entrySet()) {
            System.out.println(e);
        }
        for (Collection<String> c : index.values()) {
            System.out.println(c);
        }
        for (List<? extends CharSequence> l : index.values()) {
            System.out.println(l);
        }
        for (Object o : sinks) {
            System.out.println(o);
        }
        for (Number n : nums) {
            System.out.println(n);
        }
        for (Comparable<?> c : index.keySet()) {
            System.out.println(c);
        }
    }

    void assignments(List<String> names) {
        List<? extends CharSequence> a;
        a = names;
        Collection<String> b;
        b = new ArrayList<String>();
        var c = new ArrayList<String>();
        List<String> d = c;
        var e = names.stream().map(String::length).collect(Collectors.toList());
        List<Integer> f = e;
        for (List<String> group : List.of(names)) {
            Collection<String> g = group;
            System.out.println(g);
        }
        names.forEach(n -> {
            List<String> h = Collections.singletonList(n);
            System.out.println(h);
        });
        Object o = names;
        if (o instanceof List<?> list) {
            List<?> i = list;
            System.out.println(i);
        }
        System.out.println("" + a + b + d + f);
    }
}
//? diag none
