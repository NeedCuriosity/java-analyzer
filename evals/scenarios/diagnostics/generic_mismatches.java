import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.Optional;

class Main {
    List<String> names = new ArrayList<>();

    String ret() {
        return /*@m1*/names;
    }

    void locals(Map<String, Integer> m, Optional<String> o, Object obj) {
        String s = /*@m2*/names;
        int x = /*@m3*/m;
        List<String> l = /*@m4*/"x";
        String t = /*@m5*/o;
        List<String> u = /*@m6*/obj;
        names = /*@m7*/"y";
        for (int n : /*@m8*/names) {
            System.out.println(n);
        }
        System.out.println(s + x + l + t + u);
    }

    void fine(Map<String, Integer> m, List<? extends Number> nums) {
        Object o = names;
        Iterable<String> it = names;
        List<String> empty = Collections.emptyList();
        String first = names.isEmpty() ? "" : names.get(0);
        for (Number n : nums) {
            System.out.println(n);
        }
        for (Map.Entry<String, Integer> e : m.entrySet()) {
            System.out.println(e);
        }
        for (Object any : names) {
            System.out.println(any);
        }
        System.out.println(o.hashCode() + it.hashCode() + empty.size() + first);
    }
}
//? diag type-mismatch m1
//? diag type-mismatch m2
//? diag type-mismatch m3
//? diag type-mismatch m4
//? diag type-mismatch m5
//? diag type-mismatch m6
//? diag type-mismatch m7
//? diag type-mismatch m8
//? diag only type-mismatch
