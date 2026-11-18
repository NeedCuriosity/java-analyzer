import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

abstract class Ordering<T> {
    static <C extends Comparable<?>> Ordering<C> natural() { return null; }
    static <T> Ordering<T> allEqual() { return null; }
    abstract Ordering<T> reverse();
    abstract T min(T a, T b);
}

class Main {
    void run() {
        var /*@empty*/empty = List.of();
        var /*@map*/map = Map.of();
        var /*@list*/list = new ArrayList<>();
        var /*@sized*/sized = new HashMap<>(16);
        var /*@natural*/natural = Ordering.natural();
        var /*@none*/none = Collections.emptyList();
        var /*@first*/first = List.of().get(0);
        var /*@paren*/paren = (new ArrayList<>()).get(0);
        list.add("x");
        sized.put("k", 1);
        System.out.println(empty.size() + map.size() + none.size() + first.hashCode() + paren.hashCode());
        System.out.println(natural.reverse());
    }
}
//? hover empty ~ "List<Object> empty"
//? hover map ~ "Map<Object, Object> map"
//? hover list ~ "ArrayList<Object> list"
//? hover sized ~ "HashMap<Object, Object> sized"
//? hover natural ~ "Ordering<Comparable<?>> natural"
//? hover none ~ "List<Object> none"
//? hover first ~ "Object first"
//? hover paren ~ "Object paren"
//? diag none
