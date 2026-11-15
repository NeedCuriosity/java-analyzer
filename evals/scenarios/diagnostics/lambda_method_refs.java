import java.util.ArrayList;
import java.util.List;
import java.util.function.Consumer;
import java.util.function.Function;
import java.util.function.Supplier;

class Main {
    private List<String> field = new ArrayList<>();

    static String shout(String s) {
        return s.toUpperCase();
    }

    void uses(List<String> names) {
        List<String> sink = new ArrayList<>();
        Function<String, String> a = /*@unbound*/s -> s.trim();
        Function<String, String> b = /*@static_call*/s -> shout(s);
        Supplier<List<String>> c = /*@ctor*/() -> new ArrayList<>();
        Consumer<String> d = /*@bound_local*/x -> sink.add(x);
        Consumer<String> e = x -> field.add(x);
        Function<String, Integer> f = s -> s.length() + 1;
        Function<String, String> g = s -> s.substring(1);
        Consumer<String> h = /*@this_call*/x -> {
            log(x);
        };
        Function<Integer, String> i = n -> Integer.toString(n);
        System.out.println(a.apply("x") + b + c + d + e + f + g + h + i);
    }

    void log(String s) {
    }
}
//? diag lambda-to-method-ref unbound
//? diag lambda-to-method-ref static_call
//? diag lambda-to-method-ref ctor
//? diag lambda-to-method-ref bound_local
//? diag lambda-to-method-ref this_call
//? diag only lambda-to-method-ref
//? assist lambda_to_method_ref unbound ; has "Function<String, String> a = String::trim;"
