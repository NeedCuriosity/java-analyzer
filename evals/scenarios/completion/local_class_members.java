import java.util.ArrayList;

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
}
//? complete inside has ownField ownMethod use size add localVar param
//? complete inside lacks outerField
//? complete dot has ownField ownMethod use size
//? complete anon has anonField helper run localVar param
//? complete anon lacks outerField
