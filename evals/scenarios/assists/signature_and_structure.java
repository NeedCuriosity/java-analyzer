//- /app/Base.java
package app;

public class Base {
    protected int counter;

    void existing() {}
}
//- /app/Child.java
package app;

import java.util.ArrayList;
import java.util.List;

public class Child extends Base {
    private int own;

    private List<String> /*@pull*/collect(int limit) {
        List<String> out = new ArrayList<>();
        for (int i = 0; i < limit; i++) {
            out.add("x" + counter);
        }
        return out;
    }

    int /*@uses_own*/usesOwn() { return own; }

    void /*@clash*/existing() {}

    private static int area(int /*@w*/width, int /*@h*/height, String /*@l*/label) {
        return width * height + label.length();
    }

    int run(int a) {
        return area(a, 2, "x") + area(1, a + 1, "y") + collect(a).size();
    }

    Runnable task() {
        return /*@anon*/new Runnable() {
            @Override
            public void run() {
                System.out.println("hi");
            }
        };
    }

    java.util.Comparator<String> order() {
        return /*@anon_ret*/new java.util.Comparator<String>() {
            public int compare(String a, String b) {
                return a.length() - b.length();
            }
        };
    }

    java.util.function.Supplier<String> block() {
        return /*@anon_block*/new java.util.function.Supplier<String>() {
            public String get() {
                String s = "a";
                return s + s;
            }
        };
    }

    Object notFunctional() {
        return /*@anon_this*/new Runnable() {
            public void run() {
                System.out.println(this);
            }
        };
    }
}
//? assist pull_up_method pull ; has "    void existing() {}\n\n    protected List<String> collect(int limit) {\n        List<String> out = new ArrayList<>();" ; has "import java.util.ArrayList;\nimport java.util.List;\n\npublic class Base" ; has "private int own;\n\n    int usesOwn()"
//? noassist pull_up_method uses_own
//? noassist pull_up_method clash
//? assist move_parameter_right w ; has "area(int height, int width, String label)" ; has "area(2, a, \"x\") + area(a + 1, 1, \"y\")"
//? assist move_parameter_left l ; has "area(int width, String label, int height)" ; has "area(a, \"x\", 2)"
//? noassist move_parameter_left w
//? noassist move_parameter_right l
//? assist anonymous_to_lambda anon ; has "return () -> System.out.println(\"hi\");"
//? assist anonymous_to_lambda anon_ret ; has "return (a, b) -> a.length() - b.length();"
//? assist anonymous_to_lambda anon_block ; has "return () -> {\n            String s = \"a\";\n            return s + s;\n        };"
//? noassist anonymous_to_lambda anon_this
