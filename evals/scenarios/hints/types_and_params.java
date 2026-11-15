import java.util.List;
import java.util.Map;

class Main {
    void move(int x, int y, String label) {}
    void all(String first, int... rest) {}

    void run(List<String> names, Map<String, Integer> ages) {
        var first/*@first*/ = names.get(0);
        var size/*@size*/ = names.size();
        var entries/*@entries*/ = ages.entrySet();
        var obvious/*@obvious*/ = new StringBuilder();
        move(/*@x*/1, /*@y*/2, /*@label*/"a");
        move(size, size, first);
        all(/*@fst*/"a", /*@rest*/1, 2);
        for (var name/*@name*/ : names) {
            System.out.println(name);
        }
    }
}
//? hint first ": String"
//? hint size ": int"
//? hint entries ": Set<Map.Entry<String, Integer>>"
//? hint x "x:"
//? hint y "y:"
//? hint label "label:"
//? hint fst "first:"
//? hint rest "...rest:"
//? hint name ": String"
