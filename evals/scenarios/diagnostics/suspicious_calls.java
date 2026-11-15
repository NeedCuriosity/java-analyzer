import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;

class Main {
    interface Shape {}
    static final class Circle {}
    static class Square {}

    void equality(Integer i, Long l, String s, StringBuilder sb, Shape shape, Circle circle, Square square, Object o, CharSequence cs) {
        i.equals(/*@int_long*/l);
        s.equals(/*@string_builder*/sb);
        Objects.equals(s, /*@objects*/i);
        shape.equals(/*@final_class*/circle);
        shape.equals(square);
        s.equals(o);
        s.equals(cs);
        i.equals(5);
        i.equals(/*@boxed*/5L);
    }

    void collections(List<String> names, Set<Integer> ids, Map<String, Integer> scores, List<? extends Number> numbers) {
        names.contains(/*@contains*/42);
        names.indexOf("x");
        ids.remove(/*@set_remove*/"1");
        scores.get(/*@map_get*/1);
        scores.containsKey("a");
        scores.containsValue(/*@map_value*/"a");
        scores.getOrDefault(/*@default*/'c', 0);
        numbers.contains(1);
        names.remove(0);
    }
}
//? diag suspicious-call int_long
//? diag suspicious-call string_builder
//? diag suspicious-call objects
//? diag suspicious-call final_class
//? diag suspicious-call boxed
//? diag suspicious-call contains
//? diag suspicious-call set_remove
//? diag suspicious-call map_get
//? diag suspicious-call map_value
//? diag suspicious-call default
//? diag only suspicious-call
