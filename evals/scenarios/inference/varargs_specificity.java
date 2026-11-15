import java.util.List;

final class Main {
    static String[] parse(String... args) {
        return /*@call*/parse(List.of(args)).toArray(String[]::new);
    }

    static List<String> parse(List<String> args) {
        return args;
    }

    void f(String[] a) {
        var x = List./*@of*/of(a);
        var y = /*@y*/x;
    }
}
//? hover call ~ "List<String> parse"
//? hover of ~ "of(E... elements)"
//? hover y ~ "List<String> x"
