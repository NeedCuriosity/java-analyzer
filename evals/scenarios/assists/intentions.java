import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.function.Function;
import java.util.function.Supplier;

class Main {
    private int base;

    int braces(int a) {
        /*@braces*/if (a > 0) return 1;
        else return 2;
    }

    void split() {
        int /*@split*/count = compute();
        System.out.println(count);
    }

    void join() {
        String /*@join*/label;
        label = "x" + base;
        System.out.println(label);
    }

    boolean flip(int a, int b, String s, String t) {
        return a /*@flip*/< b && s./*@flipeq*/equals(t);
    }

    int ternary(boolean c) {
        return c /*@tern*/? 1 : 2;
    }

    int assignTernary(boolean c) {
        int /*@tdecl*/value = c ? 10 : 20;
        return value;
    }

    int toTernary(boolean c) {
        /*@toTern*/if (c) {
            return 3;
        } else {
            return 4;
        }
    }

    void refs(List<String> names) {
        names.forEach(/*@ref1*/n -> System.out.println(n));
        Function<String, Integer> len = /*@ref2*/s -> s.length();
        Supplier<List<String>> make = /*@ref3*/() -> new ArrayList<>();
        Function<String, String> self = /*@ref4*/x -> wrap(x);
        names.sort(Comparator.comparing(/*@back*/String::length));
        System.out.println(len.apply("a") + make.get().size() + self.apply("b"));
    }

    String wrap(String x) {
        return "[" + x + "]";
    }

    int sum(int[] values, List<String> words) {
        int total = 0;
        /*@efor*/for (int i = 0; i < values.length; i++) {
            total += values[i];
        }
        /*@lfor*/for (int i = 0; i < words.size(); i++) {
            System.out.println(words.get(i).trim());
        }
        /*@nofor*/for (int i = 0; i < values.length; i++) {
            System.out.println(i + values[i]);
        }
        return total;
    }

    String sw(int day) {
        String name;
        /*@sw*/switch (day) {
            case 1:
            case 7:
                name = "weekend";
                break;
            case 2:
                System.out.println("monday");
                name = "start";
                break;
            default:
                throw new IllegalArgumentException();
        }
        return name;
    }

    void fallthrough(int day) {
        /*@nosw*/switch (day) {
            case 1:
                System.out.println("a");
            case 2:
                System.out.println("b");
                break;
        }
    }

    int compute() {
        return base;
    }
}

class Point {
    Point(int /*@field*/x) {
    }
}
//? assist add_braces braces ; has "if (a > 0) {\n            return 1;\n        } else {\n            return 2;\n        }"
//? assist split_declaration split ; has "int count;\n        count = compute();"
//? assist join_declaration join ; has "String label = \"x\" + base;" ; lacks "label;"
//? assist flip_binary flip ; has "b > a && s.equals(t)"
//? assist flip_equals flipeq ; has "t.equals(s)"
//? assist ternary_to_if tern ; has "if (c) {\n            return 1;\n        } else {\n            return 2;\n        }"
//? assist ternary_to_if tdecl ; has "int value;\n        if (c) {\n            value = 10;\n        } else {\n            value = 20;\n        }"
//? assist if_to_ternary toTern ; has "return c ? 3 : 4;"
//? assist lambda_to_method_ref ref1 ; has "names.forEach(System.out::println);"
//? assist lambda_to_method_ref ref2 ; has "len = String::length;"
//? assist lambda_to_method_ref ref3 ; has "make = ArrayList::new;"
//? assist lambda_to_method_ref ref4 ; has "self = this::wrap;"
//? assist method_ref_to_lambda back ; has "Comparator.comparing(string -> string.length())"
//? assist convert_to_enhanced_for efor ; has "for (int value : values) {\n            total += value;"
//? assist convert_to_enhanced_for lfor ; has "for (String word : words) {\n            System.out.println(word.trim());"
//? noassist convert_to_enhanced_for nofor
//? assist switch_to_arrow sw ; has "case 1, 7 -> name = \"weekend\";" ; has "case 2 -> {\n                System.out.println(\"monday\");\n                name = \"start\";\n            }" ; has "default -> throw new IllegalArgumentException();" ; lacks "name = \"start\";\n                break;"
//? noassist switch_to_arrow nosw
//? assist field_from_parameter field ; has "private final int x;" ; has "this.x = x;"
