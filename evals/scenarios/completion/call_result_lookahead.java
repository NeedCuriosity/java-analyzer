import java.util.ArrayList;
import java.util.Collection;
import java.util.List;
import java.util.Map;

class Wrapper {
    private final Map<String, Integer> map = Map.of();

    int count() { return map.size(); }
    boolean empty() { return map.isEmpty(); }
    Map<String, Integer> delegate() { return map; }

    void reset() {
        /*@call_then_member*/().clear();
    }

    int lookup(String key) {
        return /*@call_with_args*/(key).length();
    }

    String normalize(String key) { return key.trim(); }
    int size() { return 0; }
}

abstract class Shape {
    abstract double area();

    static final class Square extends Shape {
        double area() { return 1; }
    }

    static Shape unit() {
        return new /*@new_nested*/();
    }
}

class Bag {
    Collection<String> items() {
        return new /*@new_list*/();
    }

    String join(int len) {
        /*@init_type*/ buf = new StringBuilder(len);
        /*@init_super*/<String> names = new ArrayList<>();
        return buf.toString() + names;
    }
}
//? complete call_then_member first delegate
//? complete call_with_args first normalize
//? complete new_nested first Square
//? complete new_list first ArrayList
//? complete init_type first StringBuilder
//? complete init_super order List Map
