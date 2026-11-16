import java.io.IOException;
import java.io.Serializable;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Collections;
import java.util.Comparator;
import java.util.HashMap;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.function.Supplier;

public class RawAndUnchecked<E> {
    private final E[] elements;
    private final List rawField = new ArrayList();
    private final List<String> typedField = new ArrayList<>();
    private static final List EMPTY = Collections.EMPTY_LIST;
    private final int size;
    private final int doubled;

    @SuppressWarnings("unchecked")
    RawAndUnchecked(int capacity) {
        elements = (E[]) new Object[capacity];
        size = capacity;
        doubled = size * 2;
    }

    static class RawList extends ArrayList {
        void addAll() {
            add("x");
        }
    }

    static class Holder<T> {
        T value;
        List<T> values = new ArrayList<>();

        T get() {
            return value;
        }

        List<T> all() {
            return values;
        }

        void put(List<T> more) {
            values.addAll(more);
        }
    }

    interface Named {
        String name();
    }

    static class NamedList<T> extends ArrayList<T> implements Named {
        public String name() {
            return "list";
        }
    }

    static <T extends Comparable<T>> void sortAll(List<T> list) {
        Collections.sort(list);
    }

    static List rawFactory() {
        return new ArrayList();
    }

    static List<String> typedFactory() {
        return new ArrayList<>();
    }

    List<String> conversions(List raw, Collection rawColl, Object object) {
        List<String> a = raw;
        List<String> b = new ArrayList();
        List<?> c = raw;
        List d = raw;
        List<String> e = rawFactory();
        List<String> f = typedFactory();
        List<String> g = EMPTY;
        List<String> h = new RawList();
        Collection<String> i = rawColl;
        Iterator<String> it = raw.iterator();
        a = raw;
        typedField.addAll(raw);
        if (a.isEmpty() && b.isEmpty() && c.isEmpty() && d.isEmpty() && e.isEmpty()) {
            return raw;
        }
        System.out.println(f.size() + g.size() + h.size() + i.size() + (it.hasNext() ? 1 : 0));
        return f;
    }

    void calls(List raw, Map rawMap, Holder rawHolder, Holder<String> holder, RawList rawList) {
        raw.add("x");
        raw.add(0, "y");
        raw.size();
        raw.contains("z");
        raw.equals(rawMap);
        rawMap.put("k", "v");
        rawMap.get("k");
        rawMap.containsKey("k");
        rawHolder.put(raw);
        rawHolder.get();
        holder.put(raw);
        holder.put(new ArrayList<String>());
        rawList.add("w");
        rawList.addAll();
        Collections.sort(raw);
        sortAll(raw);
        List copy = new ArrayList(raw);
        List sized = new ArrayList(10);
        Map copyMap = new HashMap(rawMap);
        Object[] array = raw.toArray(new String[0]);
        System.out.println(copy.size() + sized.size() + copyMap.size() + array.length);
    }

    @SuppressWarnings("unchecked")
    void suppressed(List raw) {
        raw.add("x");
        List<String> typed = raw;
        System.out.println(typed);
    }

    <T> T casts(Object object, List<String> strings, Collection<String> collection, Serializable serializable, E element) {
        String s = (String) object;
        List<String> a = (List<String>) object;
        List<?> b = (List<?>) object;
        List c = (List) object;
        ArrayList<String> d = (ArrayList<String>) strings;
        List<String> e = (List<String>) collection;
        T f = (T) object;
        E g = (E) element;
        Object h = (Object) element;
        Comparable<String> k = (Comparable<String>) s;
        ArrayList<String> l = (ArrayList<String>) serializable;
        Map.Entry<?, ?> m = (Map.Entry<?, ?>) object;
        T[] n = (T[]) new Object[3];
        System.out.println(s + a + b + c + d + e + g + h + k + l + m + n.length);
        return f;
    }

    String typed(Holder<String> holder, Supplier<String> supplier, Comparator<String> order) {
        String a = holder.get();
        List<String> b = holder.all();
        String c = supplier.get();
        int d = order.compare(a, c);
        NamedList<String> named = new NamedList<>();
        named.add(a);
        return a + b + c + d + named.name() + rawField.size() + elements.length + doubled;
    }

    void throwing(Supplier<IOException> supplier) throws IOException {
        if (supplier == null) {
            throw (IOException) new Exception().getCause();
        }
        throw supplier.get();
    }

    public static void main(String[] args) throws IOException {
        RawAndUnchecked<String> sample = new RawAndUnchecked<>(4);
        sample.conversions(new ArrayList(), new ArrayList(), "x");
        sample.calls(new ArrayList(), new HashMap(), new Holder(), new Holder<>(), new RawList());
        sample.casts(new ArrayList<String>(), new ArrayList<>(), new ArrayList<>(), new ArrayList<String>(), "e");
        sample.typed(new Holder<>(), () -> "s", Comparator.naturalOrder());
        sample.throwing(IOException::new);
    }
}
