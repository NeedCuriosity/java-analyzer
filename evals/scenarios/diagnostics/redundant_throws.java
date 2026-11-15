import java.io.FileNotFoundException;
import java.io.FileReader;
import java.io.IOException;
import java.util.concurrent.Callable;

class Main {
    private void nothing() throws /*@io*/IOException {
        System.out.println("x");
    }

    private void reads(String path) throws IOException {
        new FileReader(path).close();
    }

    private void narrower(String path) throws IOException {
        new FileReader(path);
    }

    private static void mixed(String path) throws FileNotFoundException, /*@inter*/InterruptedException {
        new FileReader(path);
    }

    private void caught(String path) throws /*@caught*/IOException {
        try {
            new FileReader(path).close();
        } catch (IOException e) {
            System.out.println(e);
        }
    }

    private void inLambda() throws /*@lambda*/Exception {
        Callable<String> c = () -> {
            throw new Exception();
        };
    }

    private void unchecked() throws IllegalStateException {
        System.out.println("y");
    }

    public void overridable() throws IOException {
        System.out.println("z");
    }

    private void rethrow(Exception e) throws Exception {
        throw e;
    }

    private void readObject(java.io.ObjectInputStream in) throws IOException, ClassNotFoundException {
        throw new java.io.InvalidObjectException("proxy");
    }

    static void packagePrivate() throws /*@pkg*/IOException {
    }

    private void thrown() throws IOException {
        throw new FileNotFoundException();
    }
}
//? diag redundant-throws io
//? diag redundant-throws inter
//? diag redundant-throws caught
//? diag redundant-throws lambda
//? diag redundant-throws pkg
//? diag only redundant-throws
//? assist remove_redundant_throws io ; has "private void nothing() {"
//? assist remove_redundant_throws inter ; has "throws FileNotFoundException {"
