import java.util.List;
import java.util.Map;

class Main<T> {
    T value;
    static /*@t1*/T shared;
    static List</*@t2*/T> empty() {
        return null;
    }
    static <T> T identity(T t) {
        return t;
    }
    static class Nested {
        /*@t3*/T item;
    }
    static class Own<T> {
        T item;
    }
    class Inner {
        T item;
    }
    enum Strength {
        STRONG {
            @Override
            <T> T wrap(T value) {
                return value;
            }
        };

        abstract <T> T wrap(T value);
    }
    void instance() {
        T local = value;
        System.out.println(local);
    }

    List</*@p1*/int> numbers;
    Map/*@p2*/<String> lookup;
    List<int[]> rows;
    Map<String, List<Integer>> groups;
    Map.Entry<String, Integer> entry;

    static class Base {
        Base(int x) {}
    }
    static class Defaults {
        Defaults(String... parts) {}
    }
    static class /*@s1*/Implicit extends Base {}
    static class Explicit extends Base {
        /*@s2*/Explicit() {}
    }
    static class Calls extends Base {
        Calls() {
            super(1);
        }
        Calls(int x) {
            this();
        }
    }
    static class Varargs extends Defaults {}
}
//? diag static-context t1
//? diag static-context t2
//? diag static-context t3
//? diag only static-context
//? diag type-mismatch p1
//? diag type-mismatch p2
//? diag only type-mismatch
//? diag argument-mismatch s1
//? diag argument-mismatch s2
//? diag only argument-mismatch
//? diag exact
