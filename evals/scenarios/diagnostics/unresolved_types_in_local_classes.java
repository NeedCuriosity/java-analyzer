import java.util.AbstractMap;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

class Main {
    static class Base {
        static class Nested {}
    }

    <T> void generic(T value) {
        class Earlier {}
        class Box<U> extends Base {
            /*@bad_field*/Missing missing;
            U u;
            T t;
            Inner inner;
            Nested nested;
            Earlier earlier;
            class Inner {}
            List<U> list() {
                /*@bad_local*/Nada x = null;
                return new ArrayList<U>();
            }
            <V> V conv(V v) {
                V w = v;
                return w;
            }
        }
        Runnable r = new Runnable() {
            /*@bad_anon_field*/Gone gone;
            public void run() {
                /*@bad_anon_local*/Absent a = null;
                T copy = value;
                Earlier e = new Earlier();
                Box<String> b = null;
            }
        };
        Map<String, String> m = new HashMap<String, String>() {
            Entry<String, String> first;
            Set<Entry<String, String>> all() { return entrySet(); }
        };
        abstract class MyMap extends AbstractMap<String, String> {
            Entry<String, String> first() { return null; }
        }
        Comparator<String> c = new Comparator<>() {
            public int compare(String a, String b) { return 0; }
        };
    }

    enum Op {
        PLUS {
            /*@bad_enum_body*/Unknown u;
            int apply(int a, int b) { Op other = MINUS; return a + b; }
        },
        MINUS {
            int apply(int a, int b) { return a - b; }
        };
        abstract int apply(int a, int b);
    }
}
//? diag unresolved-type bad_field
//? diag unresolved-type bad_local
//? diag unresolved-type bad_anon_field
//? diag unresolved-type bad_anon_local
//? diag unresolved-type bad_enum_body
//? diag exact
