import java.util.List;

class Main {
    String /*@first*/first(String s) { return s; }
    int second(int n) { return n; }
    int /*@third*/third(int m) { return m; }
    List<String> /*@names*/names(List<String> xs) { return xs; }
    List<Integer> numbers(List<Integer> xs) { return xs; }
    <T> T /*@pick*/pick(T value) { return value; }
    Object any(Object value) { return value; }
    String run() { return first("a") + second(1) + third(2) + names(null) + pick("x"); }
}
//? rename first second ; has "String second(String s)" ; has "return second(\"a\") + second(1)"
//? rename third second ; fails
//? rename names numbers ; fails
//? rename pick any ; fails
