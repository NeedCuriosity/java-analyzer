import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;
import java.util.Map;

class Bounds {
    <T extends Number> T id(T value) { return value; }

    <T extends Comparable<T>> T max(T a, T b) { return a.compareTo(b) > 0 ? a : b; }

    <E extends Enum<E>> E first(Class<E> type) { return type.getEnumConstants()[0]; }

    enum Color { RED }

    String bad1() { return String.valueOf(/*@b1*/id("wrong")); }

    Object bad2() { return /*@b2*/max(new Object(), new Object()); }

    void bad3() { List<String> value = /*@b3*/List.of(1); }

    List<String> bad4() { return /*@b4*/List.of(2, 3); }

    void bad5() { List<String> xs = /*@b5*/new ArrayList<Integer>(); }

    int ok1() { return id(1).intValue(); }

    Number ok2() { return id(2.5); }

    String ok3() { return max("a", "b"); }

    Color ok4() { return first(Color.class); }

    List<Object> ok5() { return List.of(1, 2); }

    List<? extends Number> ok6() { return List.of(1, 2.0); }

    List<String> ok7() { return new ArrayList<>(); }

    List<Integer> ok8() { return List.of(); }

    List<Number> ok9() { return new ArrayList<Number>(List.of(1, 2)); }

    <U extends Number> U ok10(U u) { return id(u); }

    void ok11(List<String> xs) {
        Collections.sort(xs);
        xs.sort(Comparator.naturalOrder());
        Map<String, List<Integer>> m = Map.of("a", List.of(1));
        List<CharSequence> cs = List.of("a", "b");
        Comparable<String> c = max("x", "y");
        System.out.println(m.size() + cs.size() + c.hashCode());
    }
}
//? diag argument-mismatch b1
//? diag argument-mismatch b2
//? diag type-mismatch b3
//? diag type-mismatch b4
//? diag type-mismatch b5
//? diag exact
