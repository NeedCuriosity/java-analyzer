import java.util.List;

class Main {
    int run(List<String> names, boolean flag, int unused) {
        var /*@var*/first = names.get(0);
        /*@if*/if (flag) {
            return 1;
        } else {
            return first.length();
        }
    }

    private int helper(int a, int /*@param*/dead) { return a; }

    int use() { return helper(1, 2); }

    int explicit() {
        String /*@typed*/s = "x";
        return s.length();
    }
}
//? assist replace_var_with_type var ; has "String first = names.get(0);"
//? assist invert_if if ; has "if (!flag) {\n            return first.length();\n        } else {\n            return 1;\n        }"
//? assist remove_unused_parameter param ; has "private int helper(int a) { return a; }" ; has "return helper(1);"
//? noassist replace_var_with_type typed
//? noassist invert_if var
