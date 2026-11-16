import java.io.IOException;

class Main {
    void run(int x) throws Exception {
        try {
            work();
        } /*@c1*/catch (IOException e) {
        } catch (IllegalStateException ignored) {
        } catch (RuntimeException e) {
            // nothing to do here
        } /*@f1*/finally {
        }
        if (x > 0)/*@s1*/;
        while (x > 0)/*@s2*/;
        while (x-- > 0);
        for (int i = 0; i < x; i++);
        for (int i = 0; i < x; i++) {
        }
        if (x == 3) {
            System.out.println();
        } else /*@s3*/;
    }

    void work() throws IOException {}
}
//? diag empty-block c1
//? diag empty-block f1
//? diag only empty-block
//? diag empty-statement-body s1
//? diag empty-statement-body s2
//? diag empty-statement-body s3
//? diag only empty-statement-body
