import java.util.HashMap;
import java.util.List;
import java.util.Map;

interface Shape {
    static Shape unit() {
        return null;
    }

    default Shape scaled() {
        return unit();
    }
}

class Square implements Shape {
    Shape make() {
        Shape a = Shape.unit();
        Shape b = Square./*@u1*/unit();
        return a != null ? a : b;
    }
}

class Main {
    void run(List<String> names, Object o, Map<String, String> m) {
        Map<String, Integer> ok = Map.of();
        HashMap<String, Integer> bad = HashMap./*@u2*/of();
        /*@g1*/take(names);
        take(o == null ? "" : "x");
        take(List.of("a").get(0));
        take(m.get("k"));
        System.out.println(ok.size() + bad.size());
    }

    void take(String s) {}
}
//? diag unresolved-method u1
//? diag unresolved-method u2
//? diag only unresolved-method
//? diag argument-mismatch g1
//? diag only argument-mismatch
