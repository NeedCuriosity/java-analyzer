import java.io.IOException;
import java.util.List;

class Main {
    void io() throws IOException {}

    void any() throws Exception {}

    void caller() throws Exception {
        /*@s1*/io();/*@s2*/
        System.out.println("x");
    }

    void caught() throws Exception {
        /*@t1*/try {
            any();
        } catch (Exception e) {
            System.out.println(e);
        }/*@t2*/
        any();
    }

    <T extends Comparable<T>> T generic(List<T> items) {
        T best = items.get(0);
        /*@g1*/for (T item : items) {
            if (item.compareTo(best) > 0) {
                best = item;
            }
        }/*@g2*/
        return best;
    }

    void rethrow() throws IOException {
        try {
            io();
        } catch (Exception e) {
            /*@r1*/System.out.println(e);
            throw e;/*@r2*/
        }
    }
}
//? assist extract_method s1 s2 ; has "private void extracted() throws IOException {" ; lacks "extracted() throws Exception"
//? assist extract_method t1 t2 ; has "private void extracted() {"
//? assist extract_method g1 g2 ; has "private <T extends Comparable<T>> T extracted(" ; has "best = extracted("
//? noassist extract_method r1 r2
