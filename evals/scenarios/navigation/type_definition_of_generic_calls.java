import java.util.List;
import java.util.Map;

class /*@item*/Item {}

class /*@box*/Box<T> {
    T value;
    T get() { return value; }
}

class Main {
    <T> T identity(T value) { return value; }

    String run(List<String> xs, Map<String, Item> byName, Box<Item> box) {
        Item a = byName./*@map_get*/get("a");
        Item b = /*@ident*/identity(a);
        Item c = box./*@box_get*/get();
        Item d = box./*@box_value*/value;
        Box<Item> e = /*@local*/box;
        return xs./*@list_get*/get(0) + a + b + c + d + e;
    }
}
//? typedef list_get -> lib java.lang.String
//? typedef map_get -> item
//? typedef ident -> item
//? typedef box_get -> item
//? typedef box_value -> item
//? typedef local -> box
