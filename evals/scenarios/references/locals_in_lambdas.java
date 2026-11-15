import java.util.List;
import java.util.function.IntSupplier;

class Main {
    int run(List<String> names, int /*@base*/base) {
        int /*@count*/count = 0;
        for (String name : names) {
            /*@w1*/count += name.length();
        }
        final int /*@snapshot*/snapshot = /*@r1*/count + /*@b1*/base;
        IntSupplier supplier = () -> /*@s1*/snapshot + /*@b2*/base;
        Runnable r = new Runnable() {
            public void run() { System.out.println(/*@s2*/snapshot); }
        };
        r.run();
        return supplier.getAsInt() + /*@r2*/count;
    }
}
//? refs count = w1 r1 r2
//? refs snapshot = s1 s2
//? refs base = b1 b2
//? diag none
