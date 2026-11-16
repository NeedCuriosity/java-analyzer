//? module alpha
//? module beta
//? module gamma beta
//- /alpha/src/main/java/com/acme/Config.java module:alpha
package com.acme;

public class Config {
    public int /*@alphaPort*/alphaPort = 8080;

    public static class Inner {
        public void /*@alphaOnly*/alphaOnly() {}
    }
}
//- /alpha/src/main/java/com/acme/Service.java module:alpha
package com.acme;

public interface /*@alphaService*/Service {
    void serveAlpha();
}
//- /alpha/src/main/java/com/acme/AlphaService.java module:alpha
package com.acme;

public class /*@alphaImpl*/AlphaService implements Service {
    @Override
    public void serveAlpha() {}
}
//- /alpha/src/main/java/com/acme/Application.java module:alpha
package com.acme;

public class /*@alphaApp*/Application {
    Config config = new Config();
    Service service = new AlphaService();

    public static void main(String[] args) {
        new /*@alphaNew*/Application().startAlpha();
    }

    public void startAlpha() {
        int port = config./*@useAlphaPort*/alphaPort;
        new Config.Inner()./*@useAlphaOnly*/alphaOnly();
        service.serveAlpha();
    }
}
//- /beta/src/main/java/com/acme/Config.java module:beta
package com.acme;

public class Config {
    public String /*@betaHost*/betaHost = "localhost";

    public Config withHost(String host) {
        betaHost = host;
        return this;
    }

    public static class Inner {
        public int /*@betaOnly*/betaOnly() { return 1; }
    }
}
//- /beta/src/main/java/com/acme/Service.java module:beta
package com.acme;

public interface /*@betaService*/Service {
    String serveBeta(int times);
}
//- /beta/src/main/java/com/acme/BetaService.java module:beta
package com.acme;

public class /*@betaImpl*/BetaService implements Service {
    @Override
    public String serveBeta(int times) { return "beta"; }
}
//- /beta/src/main/java/com/acme/Application.java module:beta
package com.acme;

public class /*@betaApp*/Application {
    Config config = new Config().withHost("example.org");
    Service service = new BetaService();

    public static void main(String[] args) {
        new /*@betaNew*/Application().startBeta(1);
    }

    public void /*@startBeta*/startBeta(int times) {
        String host = config./*@useBetaHost*/betaHost;
        int n = new Config.Inner()./*@useBetaOnly*/betaOnly();
        String served = service.serveBeta(times);
    }
}
//- /gamma/src/main/java/com/acme/gamma/Client.java module:gamma
package com.acme.gamma;

import com.acme./*@gammaImport*/Application;
import com.acme.Config;

class Client {
    void run() {
        /*@gammaType*/Application app = new /*@gammaNew*/Application();
        app./*@gammaCall*/startBeta(2);
        String host = new Config()./*@gammaField*/betaHost;
        int n = new Config.Inner().betaOnly();
    }
}
//? diag none
//? def useAlphaPort -> alphaPort
//? def useAlphaOnly -> alphaOnly
//? def useBetaHost -> betaHost
//? def useBetaOnly -> betaOnly
//? def gammaType -> betaApp
//? def gammaCall -> startBeta
//? def gammaField -> betaHost
//? hover useAlphaPort ~ "int alphaPort"
//? hover gammaField ~ "String betaHost"
//? refs alphaApp = alphaNew
//? refs betaApp = betaNew gammaImport gammaType gammaNew
//? refs alphaPort = useAlphaPort
//? impl alphaService -> alphaImpl
//? impl betaService -> betaImpl
//? symbol "Application" "Application" alphaApp
//? symbol "Application" "Application" betaApp
//? rename betaApp BetaApplication ; has "new BetaApplication().startBeta(1)" ; has "BetaApplication app = new BetaApplication()" ; has "new Application().startAlpha()"
