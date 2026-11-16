import java.util.ArrayList;
import java.util.function.Consumer;

class Main {
    int outerField;

    static void staticMethod(int param) {
        int localVar = 1;
        class Local extends ArrayList<String> {
            int ownField;
            int ownMethod() { return 0; }
            void use() {
                /*@inside*/ownField++;
            }
        }
        new Local()./*@dot*/ownMethod();
        Runnable r = new Runnable() {
            int anonField;
            void helper() {}
            public void run() {
                /*@anon*/helper();
            }
        };
    }

    static <Elem> void generic(Elem first) {
        final class Holder implements Consumer</*@header_param*/Elem> {
            /*@body_param*/Elem held;
            public void accept(Elem e) { held = e; }
        }
        new Holder().accept(first);
    }
}
//? complete header_param has Elem Consumer
//? complete body_param has Elem
//? complete inside has ownField ownMethod use size add localVar param
//? complete inside lacks outerField
//? complete dot has ownField ownMethod use size
//? complete anon has anonField helper run localVar param
//? complete anon lacks outerField
