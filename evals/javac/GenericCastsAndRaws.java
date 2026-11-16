import java.io.IOException;
import java.io.UncheckedIOException;
import java.lang.reflect.Array;
import java.lang.reflect.Method;
import java.util.AbstractList;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.HashMap;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.function.Function;

public class GenericCastsAndRaws<K, V> {
    static class Node<T> {
        T value;
        Node<T> next;

        Node(T value) {
            this.value = value;
        }
    }

    abstract static class Builder<B extends Builder<B>> {
        String name;

        B name(String name) {
            this.name = name;
            return self();
        }

        @SuppressWarnings("unchecked")
        B quietSelf() {
            return (B) this;
        }

        B self() {
            return (B) this;
        }
    }

    static final class PersonBuilder extends Builder<PersonBuilder> {
        int age;

        PersonBuilder age(int age) {
            this.age = age;
            return this;
        }
    }

    class Inner {
        K key;
        List<V> values = new ArrayList<>();

        V first() {
            return values.isEmpty() ? null : values.get(0);
        }
    }

    static class Fixed extends AbstractList {
        public Object get(int index) {
            return index;
        }

        public int size() {
            return 1;
        }
    }

    enum Color { RED, GREEN }

    private final Map<String, Integer> counts = new HashMap();
    private final Map<String, List> loose = new HashMap<>();
    private final K defaultKey;
    private final List<V> items;

    GenericCastsAndRaws(K defaultKey) {
        this.defaultKey = defaultKey;
        items = new ArrayList<>();
        items.clear();
    }

    Node<String> nodes(Node rawNode, Node<String> typed) {
        Node<String> a = rawNode.next;
        Node<String> b = typed.next;
        Object c = rawNode.value;
        String d = typed.value;
        System.out.println(a + "" + b + c + d);
        return b;
    }

    List<String> loose(String key) {
        List<String> found = loose.get(key);
        List<?> any = loose.get(key);
        System.out.println(any);
        return found;
    }

    void wildcards(List<? extends Number> numbers, Map<String, ? super Integer> sink, Optional<?> maybe) {
        Number n = numbers.get(0);
        sink.put("one", 1);
        Object o = maybe.orElse(null);
        Iterator<? extends Number> it = numbers.iterator();
        System.out.println(n + "" + o + it.hasNext());
    }

    @SuppressWarnings("rawtypes")
    void reflection(Object target, Class raw, Class<?> wildcard) throws ReflectiveOperationException {
        Method m = raw.getMethod("toString");
        Method w = wildcard.getMethod("toString");
        Object made = raw.isInstance(target) ? raw.cast(target) : null;
        Color red = (Color) Enum.valueOf(raw, "RED");
        Color green = Enum.valueOf(Color.class, "GREEN");
        System.out.println(m.invoke(target) + "" + w + made + red + green);
    }

    @SuppressWarnings("rawtypes")
    int compare(Object a, Object b, Optional rawOptional, Function<Object, String> f) {
        int r = ((Comparable) a).compareTo(b);
        Optional mapped = rawOptional.map(f);
        System.out.println(mapped);
        return r;
    }

    @SuppressWarnings("rawtypes")
    <T> T[] arrays(Class<T> type, List<T> list, Object[] objects, int n) {
        T[] a = (T[]) Array.newInstance(type, n);
        T[] b = (T[]) list.toArray();
        T c = (T) objects[0];
        Object[] d = (Object[]) list.toArray(new Object[0]);
        List[] raws = new List[2];
        System.out.println(c + "" + d.length + raws.length);
        return n > 0 ? a : b;
    }

    <A, B> B convert(A a, Function<A, B> f) {
        B viaFunction = f.apply(a);
        B cast = (B) a;
        return viaFunction != null ? viaFunction : cast;
    }

    <T extends Number & Comparable<T>> T bounded(Number n, T t) {
        T fromNumber = (T) n;
        T same = (T) t;
        return fromNumber.compareTo(same) > 0 ? fromNumber : same;
    }

    boolean sameKey(Object other) {
        if (!(other instanceof GenericCastsAndRaws)) {
            return false;
        }
        GenericCastsAndRaws<?, ?> that = (GenericCastsAndRaws<?, ?>) other;
        Map.Entry<K, V> entry = (Map.Entry<K, V>) other;
        return that.defaultKey == defaultKey && entry != null;
    }

    <T> List<T> localClass(Object seed) {
        class Holder {
            T held;

            T take(Object o) {
                held = (T) o;
                return held;
            }
        }
        List<T> out = new ArrayList<>();
        out.add(new Holder().take(seed));
        return out;
    }

    void fixed(Fixed fixedList) {
        fixedList.add("x");
        List<String> view = fixedList;
        Iterator<Object> it = fixedList.iterator();
        System.out.println(view.size() + "" + it.hasNext());
    }

    static <T> Optional<T> wrap(T data) {
        return Optional.ofNullable(data);
    }

    Optional<List<String>> wrapFixed() {
        return wrap(new Fixed());
    }

    Optional<List<Integer>> wrapRaw(List raw) {
        Optional<List<Integer>> wrapped = wrap(raw);
        return wrapped.isPresent() ? wrapped : wrap(new ArrayList());
    }

    static IOException failure(String message) {
        return new IOException(message);
    }

    static UncheckedIOException wrapped(IOException cause) {
        return new UncheckedIOException(cause);
    }

    void throwsCall(boolean fail) throws IOException {
        if (fail) {
            throw failure("x");
        }
    }

    void throwsUnchecked(IOException cause) {
        throw wrapped(cause);
    }

    void throwsCast(Throwable t) throws IOException {
        if (t instanceof IOException) {
            throw (IOException) t;
        }
        throw (RuntimeException) t;
    }

    void catchesCall() {
        try {
            throw failure("y");
        } catch (IOException e) {
            System.out.println(e);
        }
    }

    public static void main(String[] args) throws Exception {
        GenericCastsAndRaws<String, Integer> sample = new GenericCastsAndRaws<>("k");
        sample.nodes(new Node("a"), new Node<>("b"));
        sample.loose("k");
        sample.wildcards(new ArrayList<Integer>(), new HashMap<String, Number>(), Optional.empty());
        sample.reflection("x", String.class, String.class);
        sample.compare("a", "b", Optional.of("x"), String::valueOf);
        sample.arrays(String.class, new ArrayList<>(), new Object[] {"o"}, 1);
        sample.convert("a", String::length);
        sample.bounded(1, 2);
        sample.sameKey(sample);
        sample.localClass("seed");
        sample.fixed(new Fixed());
        sample.throwsCall(false);
        sample.throwsCast(new IllegalStateException());
        sample.catchesCall();
        PersonBuilder builder = new PersonBuilder().name("n").age(3).quietSelf();
        GenericCastsAndRaws<String, Integer>.Inner inner = sample.new Inner();
        System.out.println(builder.age + "" + inner.first() + sample.counts + sample.items + Collections.emptyList()
                + Comparator.naturalOrder());
    }
}
