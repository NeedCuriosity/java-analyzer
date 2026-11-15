import java.util.List;
import java.util.function.Function;
import java.util.function.Supplier;

class Main {
    static int /*@twice*/twice(int v) { return v * 2; }
    int /*@instance*/instance(String s) { return s.length(); }
    /*@ctor*/Main() {}

    int run(List<String> names) {
        Function<Integer, Integer> f = Main::/*@ref_static*/twice;
        Function<String, Integer> g = this::/*@ref_instance*/instance;
        Supplier<Main> h = Main::new;
        int total = 0;
        for (String /*@loop_var*/name : names) {
            total += /*@use_loop*/name.length();
        }
        names.forEach(/*@lambda_param*/item -> System.out.println(/*@use_lambda*/item.trim()));
        Function<String, Integer> k = (/*@typed_param*/text) -> {
            int /*@inner_local*/len = /*@use_typed*/text.length();
            return /*@use_inner*/len + total;
        };
        return f.apply(1) + g.apply("a") + k.apply("b") + h.get().hashCode();
    }
}
//? def ref_static -> twice
//? def ref_instance -> instance
//? def use_loop -> loop_var
//? def use_lambda -> lambda_param
//? def use_typed -> typed_param
//? def use_inner -> inner_local
//? hover use_lambda ~ "String item"
//? diag none
