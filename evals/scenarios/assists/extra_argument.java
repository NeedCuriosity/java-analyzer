//- /app/Main.java
package app;

import java.util.List;

class Main {
    void run(List<String> names) {
        int total = sum(1, /*@a1*/names, 2);
        int other = sum(3, 4);
        System.out.println(total + other);
        Util.greet("a", /*@a2*/5);
        log("x", /*@a3*/names);
    }

    private static int sum(int a, int b) {
        return a + b;
    }

    public void log(String s) {
        System.out.println(s);
    }
}

//- /app/Util.java
package app;

class Util {
    static void greet(String who) {
        System.out.println(who);
    }

    void use() {
        greet("b");
    }
}

//? assist remove_argument a1 ; has "int total = sum(1, 2);"
//? assist add_parameter a1 ; has "private static int sum(int a, List<String> names, int b) {" ; has "int other = sum(3, null, 4);"
//? assist add_parameter a2 ; has "static void greet(String who, int i) {" ; has "greet(\"b\", 0);"
//? assist remove_argument a3 ; has "log(\"x\");"
//? noassist add_parameter a3
